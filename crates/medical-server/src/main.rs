use std::path::PathBuf;

use medical_server::{AppState, ServerConfig, build_router};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let mut config = ServerConfig::from_env();
    let mut no_persist = std::env::var("MEDICAL_DISABLE_DISK_PERSIST")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let mut cli_data_dir: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--addr" => {
                if let Some(a) = args.next() {
                    config.bind_addr = a;
                }
            }
            "--data-dir" => {
                if let Some(d) = args.next() {
                    cli_data_dir = Some(PathBuf::from(d));
                }
            }
            "--no-persist" => {
                no_persist = true;
            }
            "--help" | "-h" => {
                eprintln!(
                    "medical-server — Headless High-Concurrency Medical AI Triage & Knowledge Server\n\
                     \n\
                     USAGE:\n\
                       medical-server [OPTIONS]\n\
                     \n\
                     OPTIONS:\n\
                       --addr <HOST:PORT>   Bind address (env: SERVER_ADDR, default: 127.0.0.1:8080)\n\
                       --data-dir <DIR>     Persistent data directory for Tantivy & encrypted snapshot (env: MEDICAL_DATA_DIR, default: ./data)\n\
                       --no-persist         Disable local disk persistence (pure in-memory ephemeral mode)\n"
                );
                return;
            }
            _ => {}
        }
    }

    if !no_persist {
        let dir = cli_data_dir
            .or_else(|| config.data_dir.clone())
            .unwrap_or_else(|| PathBuf::from("data"));
        config = config.with_persistent_data_dir(dir);
    }

    let bind_addr = config.bind_addr.clone();

    let state = match AppState::build(config).await {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(error = %err, "failed to initialize medical-server AppState");
            std::process::exit(1);
        }
    };

    // Background periodic encrypted snapshot auto-save (every 5s) when running with local snapshot
    if let Some(ref snap_path) = state.config.local_snapshot_path {
        info!(path = %snap_path.display(), "local AES-256-GCM snapshot persistence enabled");
        let state_bg = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(e) = state_bg.save_snapshot_now().await {
                    warn!(error = %e, "periodic snapshot save failed");
                }
            }
        });
    }

    let app = build_router(state.clone());

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
        let _ = state.save_snapshot_now().await;
        std::process::exit(1);
    }

    // Final synchronous flush on graceful shutdown (SIGINT / SIGTERM)
    match state.save_snapshot_now().await {
        Ok(Some(count)) => info!(records = count, "saved final encrypted snapshot before exit"),
        Ok(None) => {}
        Err(e) => warn!(error = %e, "failed to save final snapshot before exit"),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("shutdown signal received, draining connections and flushing persisted data");
}
