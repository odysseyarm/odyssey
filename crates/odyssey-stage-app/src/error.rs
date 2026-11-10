use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use odyssey_stage::StageError;
use serde_json::json;

#[derive(Debug)]
pub(crate) enum ApiError {
    BadRequest(String),
    Internal(String),
}

impl From<StageError> for ApiError {
    fn from(error: StageError) -> Self {
        Self::BadRequest(error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
