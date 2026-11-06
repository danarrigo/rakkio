use std::{collections::HashSet, env, net::SocketAddr, str::FromStr};

use uuid::Uuid;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Test,
    Production,
}

impl FromStr for Environment {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "development" => Ok(Self::Development),
            "test" => Ok(Self::Test),
            "production" => Ok(Self::Production),
            _ => bail!("APP_ENV must be development, test, or production"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GoogleOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub token_encryption_key: String,
}

#[derive(Debug, Clone)]
pub enum ObjectStorageConfig {
    Local {
        root: String,
    },
    S3 {
        bucket: String,
        region: String,
        endpoint: Option<String>,
        public_endpoint: Option<String>,
        access_key_id: String,
        secret_access_key: String,
        allow_http: bool,
        virtual_hosted_style: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Config {
    pub environment: Environment,
    pub bind_address: SocketAddr,
    pub database_url: String,
    pub database_max_connections: u32,
    pub cors_allowed_origin: String,
    pub app_url: String,
    pub allow_insecure_dev_auth: bool,
    pub invite_only: bool,
    pub groups_enabled: bool,
    pub trust_proxy_headers: bool,
    pub trusted_proxy_ip_header: crate::rate_limit::ProxyIpHeader,
    pub google_oauth: Option<GoogleOAuthConfig>,
    pub discord: Option<crate::discord::DiscordConfig>,
    pub object_storage: ObjectStorageConfig,
    pub max_file_size_bytes: usize,
    pub max_user_file_storage_bytes: i64,
    pub max_total_file_storage_bytes: i64,
    pub max_user_accounts: Option<i64>,
    pub admin_user_ids: HashSet<Uuid>,
    pub worker_trigger_token: Option<String>,
    pub google_calendar_webhook_url: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let environment = env::var("APP_ENV")
            .unwrap_or_else(|_| "development".to_owned())
            .parse()?;
        let bind_address = env::var("BIND_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1:3000".to_owned())
            .parse()
            .context("BIND_ADDRESS must be a socket address")?;
        let database_url = env::var("DATABASE_URL").context("DATABASE_URL must be configured")?;
        let database_max_connections = env::var("DATABASE_MAX_CONNECTIONS")
            .unwrap_or_else(|_| "10".to_owned())
            .parse()
            .context("DATABASE_MAX_CONNECTIONS must be a positive integer")?;
        if database_max_connections == 0 {
            bail!("DATABASE_MAX_CONNECTIONS must be greater than zero");
        }

        let cors_allowed_origin =
            env::var("CORS_ALLOWED_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".to_owned());
        let app_url = env::var("APP_URL").unwrap_or_else(|_| {
            cors_allowed_origin
                .split(',')
                .next()
                .unwrap_or_default()
                .trim()
                .to_owned()
        });
        let allow_insecure_dev_auth = env::var("ALLOW_INSECURE_DEV_AUTH")
            .unwrap_or_else(|_| "true".to_owned())
            .parse()
            .context("ALLOW_INSECURE_DEV_AUTH must be true or false")?;

        if environment == Environment::Production && allow_insecure_dev_auth {
            bail!("ALLOW_INSECURE_DEV_AUTH cannot be enabled in production");
        }
        let invite_only = env::var("INVITE_ONLY")
            .unwrap_or_else(|_| "false".to_owned())
            .parse()
            .context("INVITE_ONLY must be true or false")?;
        let groups_enabled = env::var("GROUPS_ENABLED")
            .unwrap_or_else(|_| "false".to_owned())
            .parse()
            .context("GROUPS_ENABLED must be true or false")?;
        let trust_proxy_headers = env::var("TRUST_PROXY_HEADERS")
            .unwrap_or_else(|_| "false".to_owned())
            .parse()
            .context("TRUST_PROXY_HEADERS must be true or false")?;
        // Render's public ingress overwrites CF-Connecting-IP but appends XFF.
        // Existing deployments must get the safe policy without an env cutover.
        let default_header = if env::var("RENDER").as_deref() == Ok("true") {
            "cf-connecting-ip"
        } else {
            "x-forwarded-for"
        };
        let trusted_proxy_ip_header = env::var("TRUSTED_PROXY_IP_HEADER")
            .unwrap_or_else(|_| default_header.to_owned())
            .parse()?;

        let google_values = [
            env_nonempty("GOOGLE_CLIENT_ID"),
            env_nonempty("GOOGLE_CLIENT_SECRET"),
            env_nonempty("GOOGLE_REDIRECT_URI"),
            env_nonempty("TOKEN_ENCRYPTION_KEY"),
        ];
        let google_oauth = match google_values {
            [
                Some(client_id),
                Some(client_secret),
                Some(redirect_uri),
                Some(token_encryption_key),
            ] => Some(GoogleOAuthConfig {
                client_id,
                client_secret,
                redirect_uri,
                token_encryption_key,
            }),
            [None, None, None, None] => None,
            _ => bail!(
                "GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET, GOOGLE_REDIRECT_URI, and TOKEN_ENCRYPTION_KEY must be configured together"
            ),
        };
        if environment == Environment::Production && google_oauth.is_none() {
            bail!("Google OAuth configuration is required in production");
        }

        let discord = match [
            env_nonempty("DISCORD_APPLICATION_ID"),
            env_nonempty("DISCORD_CLIENT_SECRET"),
            env_nonempty("DISCORD_PUBLIC_KEY"),
            env_nonempty("DISCORD_REDIRECT_URI"),
        ] {
            [
                Some(application_id),
                Some(client_secret),
                Some(public_key),
                Some(redirect_uri),
            ] => {
                if environment == Environment::Production && !redirect_uri.starts_with("https://") {
                    bail!("DISCORD_REDIRECT_URI must use HTTPS in production");
                }
                Some(crate::discord::DiscordConfig {
                    application_id,
                    client_secret,
                    public_key,
                    redirect_uri,
                })
            }
            [None, None, None, None] => None,
            _ => bail!(
                "DISCORD_APPLICATION_ID, DISCORD_CLIENT_SECRET, DISCORD_PUBLIC_KEY, and DISCORD_REDIRECT_URI must be configured together"
            ),
        };
        let object_storage = object_storage_from(environment, |name| env::var(name).ok())?;

        let max_file_size_bytes = env::var("MAX_FILE_SIZE_BYTES")
            .unwrap_or_else(|_| (25 * 1024 * 1024).to_string())
            .parse::<usize>()
            .context("MAX_FILE_SIZE_BYTES must be a positive integer")?;
        if !(1..=100 * 1024 * 1024).contains(&max_file_size_bytes) {
            bail!("MAX_FILE_SIZE_BYTES must be between 1 byte and 100 MiB");
        }
        let max_total_file_storage_bytes = env::var("MAX_TOTAL_FILE_STORAGE_BYTES")
            .unwrap_or_else(|_| (5_i64 * 1024 * 1024 * 1024).to_string())
            .parse::<i64>()
            .context("MAX_TOTAL_FILE_STORAGE_BYTES must be a positive integer")?;
        if max_total_file_storage_bytes <= 0 {
            bail!("MAX_TOTAL_FILE_STORAGE_BYTES must be greater than zero");
        }
        let max_user_file_storage_bytes = env::var("MAX_USER_FILE_STORAGE_BYTES")
            .unwrap_or_else(|_| max_total_file_storage_bytes.to_string())
            .parse::<i64>()
            .context("MAX_USER_FILE_STORAGE_BYTES must be a positive integer")?;
        if max_user_file_storage_bytes <= 0 {
            bail!("MAX_USER_FILE_STORAGE_BYTES must be greater than zero");
        }
        if max_user_file_storage_bytes > max_total_file_storage_bytes {
            bail!("MAX_USER_FILE_STORAGE_BYTES must not exceed MAX_TOTAL_FILE_STORAGE_BYTES");
        }
        let max_user_accounts = env_nonempty("MAX_USER_ACCOUNTS")
            .map(|value| value.parse::<i64>())
            .transpose()
            .context("MAX_USER_ACCOUNTS must be a positive integer")?;
        if max_user_accounts.is_some_and(|limit| limit <= 0) {
            bail!("MAX_USER_ACCOUNTS must be greater than zero");
        }
        let admin_user_ids = parse_admin_user_ids(&match env::var("ADMIN_USER_IDS") {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => String::new(),
            Err(error) => return Err(error).context("ADMIN_USER_IDS must be valid Unicode"),
        })?;
        let worker_trigger_token = env_nonempty("WORKER_TRIGGER_TOKEN");
        if worker_trigger_token
            .as_ref()
            .is_some_and(|token| token.len() < 32)
        {
            bail!("WORKER_TRIGGER_TOKEN must contain at least 32 characters");
        }
        let google_calendar_webhook_url = env_nonempty("GOOGLE_CALENDAR_WEBHOOK_URL");
        if let Some(webhook_url) = &google_calendar_webhook_url {
            let parsed = reqwest::Url::parse(webhook_url)
                .context("GOOGLE_CALENDAR_WEBHOOK_URL must be a valid URL")?;
            if environment == Environment::Production && parsed.scheme() != "https" {
                bail!("GOOGLE_CALENDAR_WEBHOOK_URL must use HTTPS in production");
            }
        }

        Ok(Self {
            environment,
            bind_address,
            database_url,
            database_max_connections,
            cors_allowed_origin,
            app_url,
            allow_insecure_dev_auth,
            invite_only,
            groups_enabled,
            trust_proxy_headers,
            trusted_proxy_ip_header,
            google_oauth,
            discord,
            object_storage,
            max_file_size_bytes,
            max_user_file_storage_bytes,
            max_total_file_storage_bytes,
            max_user_accounts,
            admin_user_ids,
            worker_trigger_token,
            google_calendar_webhook_url,
        })
    }
}

// Explicit local storage is for operators who provide a persistent absolute path.
// The injected lookup keeps configuration tests independent of process-global env.
fn object_storage_from(
    environment: Environment,
    get: impl Fn(&str) -> Option<String>,
) -> Result<ObjectStorageConfig> {
    let backend = get("FILE_STORAGE_BACKEND");
    if !matches!(backend.as_deref(), None | Some("local") | Some("s3")) {
        bail!("FILE_STORAGE_BACKEND must be local or s3");
    }
    let local_path = get("FILE_STORAGE_PATH");
    let s3_keys = [
        "S3_BUCKET",
        "S3_REGION",
        "S3_ACCESS_KEY_ID",
        "S3_SECRET_ACCESS_KEY",
        "S3_ENDPOINT",
        "S3_PUBLIC_ENDPOINT",
        "S3_ALLOW_HTTP",
        "S3_VIRTUAL_HOSTED_STYLE",
    ];
    let has_s3 = s3_keys
        .iter()
        .any(|key| get(key).is_some_and(|value| !value.trim().is_empty()));
    if backend.as_deref() == Some("local") {
        if has_s3 {
            bail!("local file storage cannot be combined with S3 configuration");
        }
        let root =
            local_path.context("FILE_STORAGE_PATH is required for explicit local storage")?;
        let path = std::path::Path::new(&root);
        if !path.is_absolute()
            || root.trim() != root
            || root.contains('\0')
            || !path
                .components()
                .any(|part| matches!(part, std::path::Component::Normal(_)))
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            bail!(
                "FILE_STORAGE_PATH must be an absolute directory below root without parent traversal"
            );
        }
        return Ok(ObjectStorageConfig::Local { root });
    }
    if local_path
        .as_ref()
        .is_some_and(|path| !path.trim().is_empty())
        && backend.as_deref() == Some("s3")
    {
        bail!("FILE_STORAGE_PATH cannot be combined with S3 configuration");
    }
    let nonempty = |name| get(name).filter(|value| !value.trim().is_empty());
    match [
        nonempty("S3_BUCKET"),
        nonempty("S3_REGION"),
        nonempty("S3_ACCESS_KEY_ID"),
        nonempty("S3_SECRET_ACCESS_KEY"),
    ] {
        [
            Some(bucket),
            Some(region),
            Some(access_key_id),
            Some(secret_access_key),
        ] => Ok(ObjectStorageConfig::S3 {
            bucket,
            region,
            endpoint: nonempty("S3_ENDPOINT"),
            public_endpoint: nonempty("S3_PUBLIC_ENDPOINT"),
            access_key_id,
            secret_access_key,
            allow_http: get("S3_ALLOW_HTTP")
                .unwrap_or_else(|| "false".to_owned())
                .parse()
                .context("S3_ALLOW_HTTP must be true or false")?,
            virtual_hosted_style: get("S3_VIRTUAL_HOSTED_STYLE")
                .unwrap_or_else(|| "false".to_owned())
                .parse()
                .context("S3_VIRTUAL_HOSTED_STYLE must be true or false")?,
        }),
        [None, None, None, None] if backend.is_none() && environment != Environment::Production => {
            Ok(ObjectStorageConfig::Local {
                root: local_path.unwrap_or_else(|| "./data/files".to_owned()),
            })
        }
        [None, None, None, None] if environment == Environment::Production && backend.is_none() => {
            bail!(
                "S3 object storage is required in production unless FILE_STORAGE_BACKEND=local is explicitly configured"
            )
        }
        _ => bail!(
            "S3_BUCKET, S3_REGION, S3_ACCESS_KEY_ID, and S3_SECRET_ACCESS_KEY must be configured together"
        ),
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

// Empty configuration denies everyone; a malformed member must never silently disappear.
fn parse_admin_user_ids(value: &str) -> Result<HashSet<Uuid>> {
    if value.trim().is_empty() {
        return Ok(HashSet::new());
    }
    value
        .split(',')
        .map(|member| {
            member
                .trim()
                .parse::<Uuid>()
                .context("ADMIN_USER_IDS must be comma-separated account UUIDs")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{Environment, ObjectStorageConfig, object_storage_from, parse_admin_user_ids};

    fn storage(
        environment: Environment,
        values: &[(&str, &str)],
    ) -> anyhow::Result<ObjectStorageConfig> {
        object_storage_from(environment, |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn production_local_storage_requires_explicit_opt_in_and_absolute_path() {
        assert!(matches!(storage(Environment::Production, &[
            ("FILE_STORAGE_BACKEND", "local"), ("FILE_STORAGE_PATH", "/data/files"),
        ]).unwrap(), ObjectStorageConfig::Local { root } if root == "/data/files"));
        for path in [
            "",
            "data/files",
            "/",
            "/data/../files",
            " /data/files",
            "/data/\0files",
        ] {
            assert!(
                storage(
                    Environment::Production,
                    &[
                        ("FILE_STORAGE_BACKEND", "local"),
                        ("FILE_STORAGE_PATH", path),
                    ]
                )
                .is_err(),
                "{path:?}"
            );
        }
        assert!(
            storage(
                Environment::Production,
                &[("FILE_STORAGE_BACKEND", "local")]
            )
            .is_err()
        );
        assert!(
            storage(
                Environment::Production,
                &[("FILE_STORAGE_PATH", "/data/files")]
            )
            .is_err()
        );
    }

    #[test]
    fn storage_rejects_unknown_incomplete_and_mixed_backends() {
        for backend in ["", "LOCAL", "filesystem", "s3"] {
            assert!(
                storage(
                    Environment::Production,
                    &[("FILE_STORAGE_BACKEND", backend)]
                )
                .is_err()
            );
        }
        for key in [
            "S3_BUCKET",
            "S3_REGION",
            "S3_ACCESS_KEY_ID",
            "S3_SECRET_ACCESS_KEY",
            "S3_ENDPOINT",
            "S3_PUBLIC_ENDPOINT",
            "S3_ALLOW_HTTP",
            "S3_VIRTUAL_HOSTED_STYLE",
        ] {
            assert!(
                storage(
                    Environment::Production,
                    &[
                        ("FILE_STORAGE_BACKEND", "local"),
                        ("FILE_STORAGE_PATH", "/data/files"),
                        (key, "configured"),
                    ]
                )
                .is_err()
            );
            for placeholder in ["", " "] {
                assert!(matches!(
                    storage(
                        Environment::Production,
                        &[
                            ("FILE_STORAGE_BACKEND", "local"),
                            ("FILE_STORAGE_PATH", "/data/files"),
                            (key, placeholder),
                        ]
                    )
                    .unwrap(),
                    ObjectStorageConfig::Local { .. }
                ));
            }
        }
        assert!(storage(Environment::Development, &[("S3_BUCKET", "partial")]).is_err());
        assert!(
            storage(
                Environment::Production,
                &[
                    ("FILE_STORAGE_BACKEND", "s3"),
                    ("FILE_STORAGE_PATH", "/data/files"),
                ]
            )
            .is_err()
        );
    }

    #[test]
    fn storage_defaults_keep_production_s3_and_development_local() {
        assert!(storage(Environment::Production, &[]).is_err());
        for environment in [Environment::Development, Environment::Test] {
            assert!(
                matches!(storage(environment, &[]).unwrap(), ObjectStorageConfig::Local { root } if root == "./data/files")
            );
            assert!(
                matches!(storage(environment, &[("FILE_STORAGE_PATH", "relative/files")]).unwrap(), ObjectStorageConfig::Local { root } if root == "relative/files")
            );
        }
        assert!(matches!(
            storage(
                Environment::Development,
                &[
                    ("S3_BUCKET", ""),
                    ("S3_REGION", ""),
                    ("S3_ACCESS_KEY_ID", ""),
                    ("S3_SECRET_ACCESS_KEY", ""),
                    ("S3_ALLOW_HTTP", "true"),
                ]
            )
            .unwrap(),
            ObjectStorageConfig::Local { .. }
        ));
        let mut s3 = vec![
            ("S3_BUCKET", "private"),
            ("S3_REGION", "us-east-1"),
            ("S3_ACCESS_KEY_ID", "synthetic"),
            ("S3_SECRET_ACCESS_KEY", "synthetic"),
        ];
        assert!(matches!(
            storage(Environment::Production, &s3).unwrap(),
            ObjectStorageConfig::S3 {
                allow_http: false,
                ..
            }
        ));
        // Legacy implicit S3 selection ignores FILE_STORAGE_PATH (as .env.example does).
        s3.push(("FILE_STORAGE_PATH", "./data/files"));
        for environment in [Environment::Development, Environment::Production] {
            assert!(matches!(
                storage(environment, &s3).unwrap(),
                ObjectStorageConfig::S3 { .. }
            ));
        }
        s3.pop();
        s3.push(("FILE_STORAGE_BACKEND", "s3"));
        assert!(matches!(
            storage(Environment::Production, &s3).unwrap(),
            ObjectStorageConfig::S3 { .. }
        ));
        s3.push(("FILE_STORAGE_PATH", " "));
        assert!(matches!(
            storage(Environment::Production, &s3).unwrap(),
            ObjectStorageConfig::S3 { .. }
        ));
        s3.pop();
        s3.push(("FILE_STORAGE_PATH", "/data/files"));
        assert!(storage(Environment::Production, &s3).is_err());
    }

    #[test]
    fn admin_allowlist_is_empty_by_default_and_strict() {
        assert!(parse_admin_user_ids("").unwrap().is_empty());
        assert!(parse_admin_user_ids("  ").unwrap().is_empty());
        let id = "00000000-0000-4000-8000-000000000001";
        let ids = parse_admin_user_ids(&format!(" {id}, {id} ")).unwrap();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&id.parse::<uuid::Uuid>().unwrap()));
        for invalid in [
            "owner@example.com",
            "not-a-uuid",
            ",",
            &format!("{id},"),
            &format!("{id},broken"),
        ] {
            assert!(parse_admin_user_ids(invalid).is_err(), "{invalid}");
        }
    }
}
