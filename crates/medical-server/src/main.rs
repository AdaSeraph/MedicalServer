use medical_server::{AppState, ServerConfig, build_router};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = ServerConfig::from_env();
    let bind_addr = config.bind_addr.clone();

    let state = AppState::build(config)
        .await
        .expect("failed to initialize medical-server AppState");
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {bind_addr}: {e}"));
    info!("medical-server listening on http://{bind_addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("medical-server exited with error");
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received, draining connections");
}
