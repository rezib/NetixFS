use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

#[derive(Debug, Copy, Clone, Serialize)]
pub(super) struct Ready {
    status: Status,
    checks: Checks,
}

#[derive(Debug, Copy, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Ready,
    NotReady,
}

#[derive(Debug, Copy, Clone, Serialize)]
pub(super) struct Checks {
    configuration: Check,
    jwt_keys: Check,
    pool: Check,
}

impl Checks {
    fn has_failures(&self) -> bool {
        [self.configuration, self.jwt_keys, self.pool]
            .iter()
            .any(Check::is_failure)
    }
}

impl IntoResponse for Checks {
    fn into_response(self) -> Response {
        let (status_code, status) = if self.has_failures() {
            (StatusCode::SERVICE_UNAVAILABLE, Status::NotReady)
        } else {
            (StatusCode::OK, Status::Ready)
        };
        (
            status_code,
            Json(Ready {
                status,
                checks: self,
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Copy, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Check {
    Ok,
    Unavailable,
}

impl Check {
    fn is_failure(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

pub(super) async fn run_checks() -> impl IntoResponse {
    Checks {
        configuration: Check::Ok,
        jwt_keys: Check::Ok,
        pool: Check::Ok,
    }
}
