use std::path::PathBuf;

use medical_mcp::McpServer;
use medical_mcp::backend::McpBackend;
use medical_mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use medical_server::ServerConfig;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // IMPORTANT: MCP stdio transport requires stdout to carry ONLY JSON-RPC frames.
    // All diagnostic logs go strictly to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let mut standalone = std::env::var("MEDICAL_MCP_MODE")
        .map(|v| v.eq_ignore_ascii_case("standalone"))
        .unwrap_or(false);
    let mut ephemeral = false;
    let mut cli_data_dir: Option<PathBuf> = None;
    let mut server_url = std::env::var("MEDICAL_SERVER_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
    let mut admin_token = std::env::var("MEDICAL_ADMIN_TOKEN")
        .unwrap_or_else(|_| "e2e-admin-secret".to_string());
    let mut api_key = std::env::var("MEDICAL_API_KEY").ok();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--standalone" => standalone = true,
            "--ephemeral" => ephemeral = true,
            "--data-dir" => {
                if let Some(d) = args.next() {
                    cli_data_dir = Some(PathBuf::from(d));
                }
            }
            "--url" => {
                if let Some(u) = args.next() {
                    server_url = u;
                }
            }
            "--admin-token" => {
                if let Some(t) = args.next() {
                    admin_token = t;
                }
            }
            "--api-key" => {
                if let Some(k) = args.next() {
                    api_key = Some(k);
                }
            }
            "--help" | "-h" => {
                eprintln!(
                    "medical-mcp — Headless Model Context Protocol (MCP) server for MedicalServer\n\
                     \n\
                     USAGE:\n\
                       medical-mcp [OPTIONS]\n\
                     \n\
                     OPTIONS:\n\
                       --standalone             Run embedded in-process MedicalServer (headless, no GUI or port needed)\n\
                       --data-dir <DIR>         Persistent data directory in standalone mode (env: MEDICAL_DATA_DIR, default: ./data)\n\
                       --ephemeral              Disable disk persistence in standalone mode (in-memory only)\n\
                       --url <URL>              Connect to running medical-server / cloud / MedicalStudio (default: http://127.0.0.1:8080)\n\
                       --admin-token <TOKEN>    Admin token for management/import endpoints (env: MEDICAL_ADMIN_TOKEN)\n\
                       --api-key <KEY>          Optional tenant API key (env: MEDICAL_API_KEY)\n"
                );
                return Ok(());
            }
            _ => {}
        }
    }

    let backend = if standalone {
        let mut cfg = ServerConfig::from_env();
        if !ephemeral {
            let dir = cli_data_dir
                .or_else(|| cfg.data_dir.clone())
                .unwrap_or_else(|| PathBuf::from("data"));
            cfg = cfg.with_persistent_data_dir(dir);
        }
        McpBackend::new_standalone(cfg).await?
    } else {
        McpBackend::new_http(server_url, admin_token, api_key)
    };

    let backend_for_exit = backend.clone();
    let server = McpServer::new(backend);

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Some(line) = reader.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response_opt = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(req) => server.handle_request(req).await,
            Err(e) => Some(JsonRpcResponse::err(
                Value::Null,
                -32700,
                format!("Parse error: {e}"),
            )),
        };

        if let Some(resp) = response_opt {
            let serialized = serde_json::to_string(&resp)?;
            stdout.write_all(serialized.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }

    // Final flush when stdin closes / Harness terminates session
    let _ = backend_for_exit.flush_if_standalone().await;

    Ok(())
}
