use std::{sync::Arc, time::Duration};

use aes_gcm::{
    Aes256Gcm,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, TokenUrl,
    basic::{
        BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse,
        BasicTokenType,
    },
};

use crate::google_oidc::verify_id_token;

use crate::config::GoogleOAuthConfig;

// Fixed Google endpoints prevent discovery or token headers from redirecting credentials/keys.
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";

type GoogleClient = oauth2::Client<
    BasicErrorResponse,
    oauth2::StandardTokenResponse<IdTokenFields, BasicTokenType>,
    BasicTokenIntrospectionResponse,
    oauth2::StandardRevocableToken,
    BasicRevocationErrorResponse,
>;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct IdTokenFields {
    id_token: String,
}

impl oauth2::ExtraTokenFields for IdTokenFields {}

#[derive(Clone)]
pub struct GoogleOAuth {
    inner: Arc<GoogleOAuthInner>,
}

struct GoogleOAuthInner {
    config: GoogleOAuthConfig,
    http_client: reqwest::Client,
    token_cipher: Aes256Gcm,
}

pub struct GoogleLoginStart {
    pub authorization_url: String,
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

pub struct GoogleCredentials {
    pub encrypted_access_token: Vec<u8>,
    pub encrypted_refresh_token: Option<Vec<u8>>,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

pub struct GoogleAccessToken {
    pub token: String,
    pub encrypted_token: Option<Vec<u8>>,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct GoogleLoginResult {
    pub subject: String,
    pub email: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub encrypted_access_token: Vec<u8>,
    pub encrypted_refresh_token: Option<Vec<u8>>,
    pub access_token_expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

impl GoogleOAuth {
    pub fn new(config: GoogleOAuthConfig) -> Result<Self> {
        let key = STANDARD
            .decode(config.token_encryption_key.trim())
            .context("TOKEN_ENCRYPTION_KEY must be valid base64")?;
        let token_cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow!("TOKEN_ENCRYPTION_KEY must decode to exactly 32 bytes"))?;
        let http_client = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .context("failed to create Google OAuth HTTP client")?;

        Ok(Self {
            inner: Arc::new(GoogleOAuthInner {
                config,
                http_client,
                token_cipher,
            }),
        })
    }

    pub async fn begin_login(&self) -> Result<GoogleLoginStart> {
        self.begin_authorization(false, false).await
    }

    pub async fn begin_calendar_connection(&self) -> Result<GoogleLoginStart> {
        self.begin_authorization(true, false).await
    }

    pub async fn begin_tasks_connection(&self) -> Result<GoogleLoginStart> {
        self.begin_authorization(false, true).await
    }

    async fn begin_authorization(
        &self,
        calendar_access: bool,
        tasks_access: bool,
    ) -> Result<GoogleLoginStart> {
        let client = GoogleClient::new(ClientId::new(self.inner.config.client_id.clone()))
            .set_auth_uri(AuthUrl::new(GOOGLE_AUTH_URL.to_owned())?)
            .set_redirect_uri(RedirectUrl::new(self.inner.config.redirect_uri.clone())?);
        let nonce = CsrfToken::new_random();
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let request = client
            .authorize_url(CsrfToken::new_random)
            .add_extra_param("nonce", nonce.secret())
            .add_scope(Scope::new("openid".to_owned()))
            .add_scope(Scope::new("email".to_owned()))
            .add_scope(Scope::new("profile".to_owned()))
            .add_extra_param("access_type", "offline");
        let request = if calendar_access {
            request
                .add_scope(Scope::new(
                    "https://www.googleapis.com/auth/calendar.events".to_owned(),
                ))
                .add_scope(Scope::new(
                    "https://www.googleapis.com/auth/calendar.calendarlist.readonly".to_owned(),
                ))
        } else {
            request
        };
        let request = if tasks_access {
            request.add_scope(Scope::new(crate::google_tasks::TASKS_SCOPE.to_owned()))
        } else {
            request
        };
        let request = if calendar_access || tasks_access {
            // Incremental consent must not discard the other enabled integration.
            request
                .add_extra_param("include_granted_scopes", "true")
                .add_extra_param("prompt", "consent")
        } else {
            request
        };
        let (authorization_url, state) = request.set_pkce_challenge(pkce_challenge).url();

        Ok(GoogleLoginStart {
            authorization_url: authorization_url.to_string(),
            state: state.secret().to_owned(),
            nonce: nonce.secret().to_owned(),
            pkce_verifier: pkce_verifier.secret().to_owned(),
        })
    }

    pub async fn complete_login(
        &self,
        code: String,
        nonce: String,
        pkce_verifier: String,
    ) -> Result<GoogleLoginResult> {
        self.complete_login_at(
            code,
            nonce,
            pkce_verifier,
            GOOGLE_TOKEN_URL,
            GOOGLE_JWKS_URL,
        )
        .await
    }

    // Endpoints are supplied only by the fixed production wrapper or offline tests.
    async fn complete_login_at(
        &self,
        code: String,
        nonce: String,
        pkce_verifier: String,
        token_url: &str,
        jwks_url: &str,
    ) -> Result<GoogleLoginResult> {
        let client = GoogleClient::new(ClientId::new(self.inner.config.client_id.clone()))
            .set_client_secret(ClientSecret::new(self.inner.config.client_secret.clone()))
            .set_token_uri(TokenUrl::new(token_url.to_owned())?)
            .set_redirect_uri(RedirectUrl::new(self.inner.config.redirect_uri.clone())?);
        let token_response = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
            .request_async(&self.inner.http_client)
            .await
            .context("Google rejected the authorization code")?;
        // Fetch only from Google's fixed TLS endpoint on each callback. No stale-key cache,
        // header-directed URL, or retry of the one-use authorization code is involved.
        let mut response = self
            .inner
            .http_client
            .get(jwks_url)
            .send()
            .await?
            .error_for_status()?;
        if response.status() != reqwest::StatusCode::OK {
            bail!("Google signing key endpoint did not return a successful response");
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len().saturating_add(chunk.len()) > 1024 * 1024 {
                bail!("Google signing key response exceeded its limit");
            }
            body.extend_from_slice(&chunk);
        }
        let jwks = serde_json::from_slice(&body).context("Google returned invalid signing keys")?;
        let claims = verify_id_token(
            &token_response.extra_fields().id_token,
            &jwks,
            &self.inner.config.client_id,
            &nonce,
            token_response.access_token().secret(),
        )?;
        let email = claims.email;
        let display_name = claims.name.unwrap_or_else(|| email.clone());
        let avatar_url = claims.picture;
        let access_token_expires_at = token_response
            .expires_in()
            .and_then(|duration| chrono::Duration::from_std(duration).ok())
            .map(|duration| Utc::now() + duration);
        let scopes = token_response
            .scopes()
            .map(|scopes| {
                scopes
                    .iter()
                    .map(|scope| scope.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default();

        Ok(GoogleLoginResult {
            subject: claims.sub,
            email,
            display_name,
            avatar_url,
            encrypted_access_token: self
                .encrypt(token_response.access_token().secret().as_bytes())?,
            encrypted_refresh_token: token_response
                .refresh_token()
                .map(|token| self.encrypt(token.secret().as_bytes()))
                .transpose()?,
            access_token_expires_at,
            scopes,
        })
    }

    pub async fn access_token(&self, credentials: &GoogleCredentials) -> Result<GoogleAccessToken> {
        let still_valid = credentials
            .access_token_expires_at
            .is_none_or(|expires_at| expires_at > Utc::now() + chrono::Duration::seconds(60));
        if still_valid {
            return Ok(GoogleAccessToken {
                token: String::from_utf8(self.decrypt(&credentials.encrypted_access_token)?)
                    .context("stored Google access token was not UTF-8")?,
                encrypted_token: None,
                expires_at: credentials.access_token_expires_at,
            });
        }
        let refresh_token = credentials
            .encrypted_refresh_token
            .as_ref()
            .ok_or_else(|| anyhow!("Google authorization has expired and cannot be refreshed"))?;
        let refresh_token = String::from_utf8(self.decrypt(refresh_token)?)
            .context("stored Google refresh token was not UTF-8")?;
        let response = self
            .inner
            .http_client
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("client_id", self.inner.config.client_id.as_str()),
                ("client_secret", self.inner.config.client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .context("failed to refresh Google access token")?
            .error_for_status()
            .context("Google rejected the refresh token")?
            .json::<RefreshTokenResponse>()
            .await
            .context("Google returned an invalid refresh response")?;
        let expires_at = Utc::now() + chrono::Duration::seconds(response.expires_in);
        Ok(GoogleAccessToken {
            encrypted_token: Some(self.encrypt(response.access_token.as_bytes())?),
            token: response.access_token,
            expires_at: Some(expires_at),
        })
    }

    pub async fn revoke(&self, credentials: &GoogleCredentials) -> Result<()> {
        let encrypted_token = credentials
            .encrypted_refresh_token
            .as_ref()
            .unwrap_or(&credentials.encrypted_access_token);
        let token = String::from_utf8(self.decrypt(encrypted_token)?)
            .context("stored Google credential was not UTF-8")?;
        let response = self
            .inner
            .http_client
            .post("https://oauth2.googleapis.com/revoke")
            .form(&[("token", token)])
            .send()
            .await
            .context("failed to revoke Google credential")?;
        if response.status().is_server_error() {
            response
                .error_for_status()
                .context("Google credential revocation is unavailable")?;
        }
        Ok(())
    }

    fn decrypt(&self, encrypted: &[u8]) -> Result<Vec<u8>> {
        if encrypted.len() <= 12 {
            bail!("stored Google credential is invalid");
        }
        let (nonce, ciphertext) = encrypted.split_at(12);
        self.inner
            .token_cipher
            .decrypt(aes_gcm::Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow!("failed to decrypt Google credential"))
    }

    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .inner
            .token_cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| anyhow!("failed to encrypt Google credential"))?;
        let mut encrypted = nonce.to_vec();
        encrypted.extend(ciphertext);
        Ok(encrypted)
    }
}

#[derive(serde::Deserialize)]
struct RefreshTokenResponse {
    access_token: String,
    expires_in: i64,
}

#[cfg(test)]
mod consent_tests {
    use super::*;
    use std::collections::HashMap;

    fn offline_google() -> GoogleOAuth {
        GoogleOAuth::new(GoogleOAuthConfig {
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            redirect_uri: "http://localhost/callback".into(),
            token_encryption_key: STANDARD.encode([7_u8; 32]),
        })
        .unwrap()
    }

    fn params(start: GoogleLoginStart) -> HashMap<String, String> {
        reqwest::Url::parse(&start.authorization_url)
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect()
    }

    #[tokio::test]
    async fn tasks_consent_is_incremental_and_not_requested_at_sign_in() {
        let google = offline_google();
        let login = params(google.begin_login().await.unwrap());
        assert!(!login["scope"].contains(crate::google_tasks::TASKS_SCOPE));
        assert!(!login["scope"].contains("calendar"));
        let tasks = params(google.begin_tasks_connection().await.unwrap());
        assert!(
            tasks["scope"]
                .split_whitespace()
                .any(|scope| scope == crate::google_tasks::TASKS_SCOPE)
        );
        assert!(!tasks["scope"].contains("calendar"));
        assert_eq!(tasks["include_granted_scopes"], "true");
        assert_eq!(tasks["prompt"], "consent");
        assert_eq!(tasks["code_challenge_method"], "S256");
    }

    #[tokio::test]
    async fn random_state_nonce_and_pkce_are_independent_and_bound_to_url() {
        let google = offline_google();
        let first = google.begin_login().await.unwrap();
        let second = google.begin_login().await.unwrap();
        assert_ne!(first.state, second.state);
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.pkce_verifier, second.pkce_verifier);
        assert_ne!(first.state, first.nonce);
        let challenge = PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
            first.pkce_verifier.clone(),
        ));
        let expected_state = first.state.clone();
        let expected_nonce = first.nonce.clone();
        let url = params(first);
        assert_eq!(url["state"], expected_state);
        assert_eq!(url["nonce"], expected_nonce);
        assert_eq!(url["code_challenge"], challenge.as_str());
        assert_eq!(url["response_type"], "code");
        assert_eq!(url["redirect_uri"], "http://localhost/callback");
    }

    #[tokio::test]
    async fn offline_code_exchange_verifies_signed_identity_and_encrypts_credentials() {
        use crate::google_oidc::tests::{claims, jwks, signed};
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        let token = signed(&claims());
        let app = Router::new()
            .route("/token", post(move |headers: axum::http::HeaderMap, axum::Form(form): axum::Form<HashMap<String, String>>| async move {
                assert!(headers.contains_key("authorization"));
                assert_eq!(form["code"], "one-use-code");
                assert_eq!(form["code_verifier"], "test-verifier");
                assert_eq!(form["grant_type"], "authorization_code");
                assert_eq!(form["redirect_uri"], "http://localhost/callback");
                Json(serde_json::json!({
                    "access_token": "access-token", "refresh_token": "refresh-token",
                    "token_type": "Bearer", "expires_in": 3600,
                    "scope": "openid email profile", "id_token": token
                }))
            }))
            .route("/keys", get(|| async { Json(jwks()) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let google = offline_google();
        let result = google
            .complete_login_at(
                "one-use-code".into(),
                "test-nonce".into(),
                "test-verifier".into(),
                &format!("http://{address}/token"),
                &format!("http://{address}/keys"),
            )
            .await
            .unwrap();
        assert_eq!(result.subject, "google-user");
        assert_eq!(result.email, "test@example.com");
        assert_eq!(result.scopes, ["openid", "email", "profile"]);
        assert_eq!(
            google.decrypt(&result.encrypted_access_token).unwrap(),
            b"access-token"
        );
        assert_eq!(
            google
                .decrypt(&result.encrypted_refresh_token.unwrap())
                .unwrap(),
            b"refresh-token"
        );
        assert_ne!(result.encrypted_access_token, b"access-token");
        assert!(result.access_token_expires_at.unwrap() > Utc::now());
        server.abort();
    }

    #[tokio::test]
    async fn offline_exchange_fails_closed_without_retrying_the_code() {
        use crate::google_oidc::tests::{claims, jwks, signed};
        use axum::{
            Router,
            body::Body,
            http::{Response, StatusCode},
            routing::{get, post},
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        for scenario in [
            "missing-id-token",
            "bad-nonce",
            "unknown-key",
            "token-error",
            "token-redirect",
            "keys-redirect",
            "oversized-keys",
        ] {
            let requests = Arc::new(AtomicUsize::new(0));
            let count = requests.clone();
            let redirects = Arc::new(AtomicUsize::new(0));
            let redirected = redirects.clone();
            let app = Router::new()
                .route(
                    "/token",
                    post(move || {
                        count.fetch_add(1, Ordering::SeqCst);
                        async move {
                            let mut identity = claims();
                            if scenario == "bad-nonce" {
                                identity["nonce"] = serde_json::json!("wrong");
                            }
                            let mut response = serde_json::json!({
                                "access_token": "access-token", "token_type": "Bearer",
                                "id_token": signed(&identity)
                            });
                            if scenario == "missing-id-token" {
                                response.as_object_mut().unwrap().remove("id_token");
                            }
                            let status = match scenario {
                                "token-error" => StatusCode::BAD_REQUEST,
                                "token-redirect" => StatusCode::TEMPORARY_REDIRECT,
                                _ => StatusCode::OK,
                            };
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .header("location", "/must-not-follow")
                                .body(Body::from(response.to_string()))
                                .unwrap()
                        }
                    }),
                )
                .route(
                    "/keys",
                    get(move || async move {
                        let mut keys = jwks();
                        if scenario == "unknown-key" {
                            keys.keys.clear();
                        }
                        let body = if scenario == "oversized-keys" {
                            " ".repeat(1024 * 1024 + 1)
                        } else {
                            serde_json::to_string(&keys).unwrap()
                        };
                        Response::builder()
                            .status(if scenario == "keys-redirect" {
                                StatusCode::TEMPORARY_REDIRECT
                            } else {
                                StatusCode::OK
                            })
                            .header("content-type", "application/json")
                            .header("location", "/must-not-follow")
                            .body(Body::from(body))
                            .unwrap()
                    }),
                )
                .route(
                    "/must-not-follow",
                    axum::routing::any(move || {
                        redirected.fetch_add(1, Ordering::SeqCst);
                        async { StatusCode::INTERNAL_SERVER_ERROR }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let result = offline_google()
                .complete_login_at(
                    "one-use-code".into(),
                    "test-nonce".into(),
                    "test-verifier".into(),
                    &format!("http://{address}/token"),
                    &format!("http://{address}/keys"),
                )
                .await;
            assert!(result.is_err(), "accepted {scenario}");
            assert_eq!(requests.load(Ordering::SeqCst), 1, "retried {scenario}");
            assert_eq!(
                redirects.load(Ordering::SeqCst),
                0,
                "followed redirect for {scenario}"
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn reconnecting_calendar_preserves_previously_granted_tasks_scope() {
        let calendar = params(offline_google().begin_calendar_connection().await.unwrap());
        assert_eq!(calendar["include_granted_scopes"], "true");
        assert!(calendar["scope"].contains("calendar.events"));
        assert!(!calendar["scope"].contains(crate::google_tasks::TASKS_SCOPE));
    }
}
