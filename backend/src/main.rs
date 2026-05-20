mod config;
mod controllers;
mod models;
mod routes;
mod services;

use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::{Database, DatabaseConfig};
use crate::controllers::auth_controller::AppState;
use crate::services::auth_service::AuthService;
use crate::services::email_service::EmailService;

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { ok: true })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            "ezpay_backend=info,tower_http=info".into()
        }))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = DatabaseConfig::from_env()?;
    let db = Database::new(config).await?;

    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "debug_secret".to_string());
    
    let auth_service = AuthService::new(db.clone(), jwt_secret);
    let email_service = EmailService::new(
        std::env::var("SMTP_SERVER").unwrap_or_default(),
        std::env::var("SMTP_USER").unwrap_or_default(),
        std::env::var("SMTP_PASS").unwrap_or_default(),
    );

    let state = Arc::new(AppState {
        auth_service,
        email_service,
    });

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3001);

    let app = Router::new()
        .route("/health", get(health))
        .nest("/auth", routes::auth_routes::auth_routes(state.clone()))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
                .max_age(Duration::from_secs(60 * 60)),
        )
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "EzPay backend listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
