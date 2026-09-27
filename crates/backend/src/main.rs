//! Axum backend service binary for the PDF Tools Suite.
//!
//! Boots the Security_Gateway-fronted Axum app (see the `backend` library
//! crate). TLS 1.2+ termination is handled by the deployment reverse proxy
//! (Req 38.1); this process asserts HTTPS in production via config.

use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let addr: SocketAddr = std::env::var("BACKEND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:8080".to_string())
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 8080)));

    tracing::info!(%addr, "backend listening");

    if let Err(e) = backend::run(addr).await {
        tracing::error!(error = %e, "backend failed to start");
        std::process::exit(1);
    }
}
