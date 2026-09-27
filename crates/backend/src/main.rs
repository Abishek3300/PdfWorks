//! Axum backend service binary for the PDF Tools Suite.
//!
//! Boots the Security_Gateway-fronted Axum app (see the `backend` library
//! crate). TLS 1.2+ termination is handled by the deployment reverse proxy
//! (Req 38.1); this process asserts HTTPS in production via config.

use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    // `--health-check`: a minimal self-check used by the Docker HEALTHCHECK
    // (docker/backend.Dockerfile). It confirms the binary links and the native
    // engine is reachable, prints `ok`, and exits 0 without binding a socket so
    // the container runtime can probe liveness cheaply. TLS stays terminated at
    // the proxy (Req 38.1) — this flag never touches the network.
    if std::env::args().any(|a| a == "--health-check") {
        if backend::health_check() {
            println!("ok");
            std::process::exit(0);
        }
        eprintln!("health check failed");
        std::process::exit(1);
    }

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
