use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Use a custom error type that can be returned by handlers.
///
/// This follows recommendations from the axum documentation:
/// <https://github.com/tokio-rs/axum/blob/main/examples/anyhow-error-response/src/main.rs>
pub struct AppError {
    pub status_code: StatusCode,
    pub message: String,
}

impl AppError {
    /// Create a new Result<T, AppError>::Err with the corresponding StatusCode and message.
    ///
    /// Useful for quickly returning a custom error in any of the request handlers.
    pub fn err<T>(status_code: StatusCode, message: impl Into<String>) -> Result<T, Self> {
        Err(Self {
            status_code,
            message: message.into(),
        })
    }

    /// Create a new AppError with the corresponding StatusCode and message.
    pub fn new(status_code: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status_code,
            message: message.into(),
        }
    }

    /// Create a new AppError with the corresponding message and StatusCode 500.
    pub fn new500(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

/// Allows axum to automatically convert our custom AppError into a Response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Report client-side errors that are not 404s as warnings.
        // This might help identify implementation problems in the frontent.
        if self.status_code.is_client_error() && self.status_code != StatusCode::NOT_FOUND {
            tracing::warn!(status_code = self.status_code.to_string(), self.message);
        // 5XXs are always severe errors, even if the server doesn't crash
        } else if self.status_code.is_server_error() {
            tracing::error!(status_code = self.status_code.to_string(), self.message);
        }
        (
            self.status_code,
            format!("{}: {}", self.status_code, self.message),
        )
            .into_response()
    }
}

/// Ensure that our custom error type can be built automatically from anyhow::Error.
/// This allows us to use the ?-operator in request-handlers to easily handle errors.
impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.into().to_string(),
        }
    }
}
