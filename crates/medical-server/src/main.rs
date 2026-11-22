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

    let state = match AppState::build(config).await {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(error = %err, "failed to initialize medical-server AppState");
            std::process::exit(1);
        }
    };
    let app = build_router(state);

    let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(err) => {
            tracing::error!(addr = %bind_addr, error = %err, "failed to bind medical-server listener");
            std::process::exit(1);
        }
    };
    info!("medical-server listening on http://{bind_addr}");

    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!(error = %err, "medical-server exited with error");
        std::process::exit(1);
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    info!("shutdown signal received, draining connections");
}
