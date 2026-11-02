use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::{
    app::AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    rate_limit::LoginRateLimiter,
};

#[derive(Clone)]
pub struct RequestLimits {
    writes: LoginRateLimiter,
    reads: LoginRateLimiter,
    requests: Arc<Semaphore>,
    authentication: Arc<Semaphore>,
    discord: Arc<Semaphore>,
    large: Arc<Semaphore>,
    accounts: Arc<Mutex<HashMap<Uuid, Arc<AccountSlots>>>>,
}

struct AccountSlots {
    requests: Arc<Semaphore>,
    writes: Arc<Semaphore>,
    large: Arc<Semaphore>,
}

struct AccountPermit {
    _account: Arc<AccountSlots>,
    _request: OwnedSemaphorePermit,
    _write: Option<OwnedSemaphorePermit>,
    _large: Option<OwnedSemaphorePermit>,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            writes: LoginRateLimiter::with_limit(240),
            reads: LoginRateLimiter::with_limit(600),
            requests: Arc::new(Semaphore::new(32)),
            authentication: Arc::new(Semaphore::new(4)),
            discord: Arc::new(Semaphore::new(4)),
            large: Arc::new(Semaphore::new(2)),
            accounts: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl RequestLimits {
    pub fn check_write(&self, user_id: Uuid) -> AppResult<()> {
        self.writes.check_key(&user_id.to_string())
    }

    pub(crate) async fn discord_admission(&self) -> AppResult<OwnedSemaphorePermit> {
        acquire(&self.discord).await
    }

    pub(crate) async fn discord_command(&self, user: Uuid, write: bool) -> AppResult<impl Sized> {
        if write {
            self.check_write(user)?;
        }
        let account = self.account_request(user, write, false).await?;
        let request = acquire(&self.requests).await?;
        Ok((account, request))
    }

    pub fn large_request(&self) -> AppResult<OwnedSemaphorePermit> {
        self.large.clone().try_acquire_owned().map_err(|_| busy())
    }

    async fn account_request(
        &self,
        user: Uuid,
        write: bool,
        large: bool,
    ) -> AppResult<AccountPermit> {
        if !write {
            self.reads.check_key(&user.to_string())?;
        }
        let account = {
            let mut accounts = self.accounts.lock().map_err(|_| busy())?;
            // Inactive accounts retain no capacity state. A live permit or
            // waiting request holds its Arc, keeping concurrent callers together.
            accounts.retain(|_, slots| Arc::strong_count(slots) > 1);
            accounts
                .entry(user)
                .or_insert_with(|| {
                    Arc::new(AccountSlots {
                        requests: Arc::new(Semaphore::new(8)),
                        writes: Arc::new(Semaphore::new(4)),
                        large: Arc::new(Semaphore::new(1)),
                    })
                })
                .clone()
        };
        let request = acquire(&account.requests).await?;
        let write = if write {
            Some(acquire(&account.writes).await?)
        } else {
            None
        };
        let large = if large {
            Some(acquire(&account.large).await?)
        } else {
            None
        };
        Ok(AccountPermit {
            _account: account,
            _request: request,
            _write: write,
            _large: large,
        })
    }
}

fn busy() -> AppError {
    AppError::InvalidRequest {
        status: StatusCode::TOO_MANY_REQUESTS,
        message: "the server is busy; try again shortly".to_owned(),
    }
}

async fn acquire(slots: &Arc<Semaphore>) -> AppResult<OwnedSemaphorePermit> {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        slots.clone().acquire_owned(),
    )
    .await
    .map_err(|_| busy())?
    .map_err(|_| busy())
}

pub async fn limit_requests(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().trim_start_matches("/api/v1");
    if matches!(
        path,
        "/auth/google/start" | "/auth/google/callback" | "/development/session"
    ) {
        let _permit = match acquire(&state.request_limits.authentication).await {
            Ok(permit) => permit,
            Err(error) => return error.into_response(),
        };
        return next.run(request).await;
    }
    let admission = match acquire(&state.request_limits.requests).await {
        Ok(permit) => permit,
        Err(error) => return error.into_response(),
    };
    let logout = request.method() == Method::POST && path == "/session/logout";
    let upload = request.method() == Method::POST && path == "/files";
    let large = upload
        || path.starts_with("/exports/")
        || path.starts_with("/imports/")
        || (request.method() == Method::GET && path == "/notes")
        || (path.starts_with("/files/") && path.ends_with("/download"));
    let write = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let (mut parts, body) = request.into_parts();
    let user = match CurrentUser::from_request_parts(&mut parts, &state).await {
        Ok(CurrentUser(user)) => user,
        Err(error) => return error.into_response(),
    };
    // Revoking a session must remain possible even at the account's write cap.
    // CurrentUser has already checked CSRF; logout reads no request body.
    if logout {
        return next.run(Request::from_parts(parts, body)).await;
    }
    // A busy account must not occupy global capacity while waiting for its own
    // slots. Authentication is cached privately for the downstream extractor.
    drop(admission);
    let _account = match state
        .request_limits
        .account_request(user, write, large)
        .await
    {
        Ok(permit) => permit,
        Err(error) => return error.into_response(),
    };
    let _permit = match acquire(&state.request_limits.requests).await {
        Ok(permit) => permit,
        Err(error) => return error.into_response(),
    };
    let _large = if large && !upload {
        match acquire(&state.request_limits.large).await {
            Ok(permit) => Some(permit),
            Err(error) => return error.into_response(),
        }
    } else {
        None
    };
    next.run(Request::from_parts(parts, body)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discord_slots_cannot_starve_public_login() {
        let limits = RequestLimits::default();
        let mut webhooks = Vec::new();
        for _ in 0..4 {
            webhooks.push(limits.discord_admission().await.unwrap());
        }
        assert!(limits.discord.clone().try_acquire_owned().is_err());
        assert!(limits.authentication.clone().try_acquire_owned().is_ok());
        drop(webhooks);
        let mut authentication = Vec::new();
        for _ in 0..4 {
            authentication.push(limits.authentication.clone().try_acquire_owned().unwrap());
        }
        assert!(limits.discord_admission().await.is_ok());
    }

    #[test]
    fn large_requests_are_bounded_across_clones_and_release_on_drop() {
        let limits = RequestLimits::default();
        let first = limits.large_request().unwrap();
        let second = limits.clone().large_request().unwrap();
        assert!(limits.large_request().is_err());
        drop(first);
        assert!(limits.large_request().is_ok());
        drop(second);
    }

    #[test]
    fn write_limit_preserves_bulk_and_undo_but_blocks_flooding() {
        let limits = RequestLimits::default();
        let user = Uuid::now_v7();
        for _ in 0..240 {
            limits.check_write(user).unwrap();
        }
        assert!(limits.check_write(user).is_err());
        assert!(limits.check_write(Uuid::now_v7()).is_ok());
    }

    #[tokio::test]
    async fn account_heavy_capacity_is_isolated_and_released() {
        let limits = RequestLimits::default();
        let user = Uuid::now_v7();
        let first = limits.account_request(user, false, true).await.unwrap();
        assert!(limits.account_request(user, false, true).await.is_err());
        assert!(
            limits
                .account_request(Uuid::now_v7(), false, true)
                .await
                .is_ok()
        );
        drop(first);
        assert!(limits.account_request(user, false, true).await.is_ok());
    }
}
