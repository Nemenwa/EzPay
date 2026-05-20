use axum::{routing::post, Router};
use crate::controllers::auth_controller::{register, AppState};
use std::sync::Arc;

pub fn auth_routes(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/register", post(register))
        .with_state(state)
}
