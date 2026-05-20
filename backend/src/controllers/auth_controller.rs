use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use crate::models::user::RegisterRequest;
use crate::services::auth_service::AuthService;
use crate::services::email_service::EmailService;
use std::sync::Arc;
use validator::Validate;

pub struct AppState {
    pub auth_service: AuthService,
    pub email_service: EmailService,
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RegisterRequest>,
) -> Result<impl IntoResponse, AppError> {
    // Validate request
    payload.validate().map_err(|e| AppError::ValidationError(e.to_string()))?;

    // Register user
    let response = state.auth_service.register(payload).await
        .map_err(|e| {
            if e.to_string().contains("already exists") {
                AppError::Conflict(e.to_string())
            } else {
                AppError::InternalServerError(e.to_string())
            }
        })?;

    // Send welcome email (asynchronously)
    let email = response.user.email.clone();
    let email_service = Arc::new(EmailService::new(
        String::new(), // smtp_server
        String::new(), // smtp_user
        String::new(), // smtp_pass
    ));
    tokio::spawn(async move {
        if let Err(e) = email_service.send_welcome_email(&email).await {
            tracing::error!("Failed to send welcome email: {}", e);
        }
    });

    Ok((StatusCode::CREATED, Json(response)))
}

pub enum AppError {
    ValidationError(String),
    Conflict(String),
    InternalServerError(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match self {
            AppError::ValidationError(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg),
            AppError::InternalServerError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(serde_json::json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}
