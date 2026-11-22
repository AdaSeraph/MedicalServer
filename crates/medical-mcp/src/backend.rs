use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, header};
use medical_server::{AppState, ServerConfig, build_router};
use serde_json::Value;
use tower::ServiceExt;

/// Backend transport for `medical-mcp`: either talks to a running `medical-server` / `MedicalStudio`
/// HTTP instance, or executes requests directly against an embedded in-process `Router`.
#[derive(Clone)]
pub enum McpBackend {
    Http {
        base_url: String,
        admin_token: String,
        api_key: Option<String>,
        client: reqwest::Client,
    },
    Standalone {
        state: Arc<AppState>,
        router: Router,
        admin_token: String,
    },
}

impl McpBackend {
    pub fn new_http(
        base_url: impl Into<String>,
        admin_token: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        Self::Http {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            admin_token: admin_token.into(),
            api_key,
            client: reqwest::Client::new(),
        }
    }

    /// Creates an in-process standalone MedicalServer backend (zero ports required).
    pub async fn new_standalone(mut config: ServerConfig) -> Result<Self, String> {
        let admin_token = config.admin_token.clone();
        if config.database_url.is_none() {
            config.offline_mode = true;
        }
        let state = AppState::build(config).await?;
        let router = build_router(state.clone());
        Ok(Self::Standalone {
            state,
            router,
            admin_token,
        })
    }

    /// Flushes the encrypted local snapshot to disk when running in standalone persistent mode.
    pub async fn flush_if_standalone(&self) -> Result<Option<usize>, String> {
        match self {
            Self::Standalone { state, .. } => state.save_snapshot_now().await,
            Self::Http { .. } => Ok(None),
        }
    }

    /// Executes an API request against MedicalServer and returns the parsed JSON response.
    pub async fn request_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        prefer_admin: bool,
    ) -> Result<Value, String> {
        match self {
            Self::Http {
                base_url,
                admin_token,
                api_key,
                client,
            } => {
                let url = format!("{base_url}{path}");
                let mut req = client.request(
                    reqwest::Method::from_bytes(method.as_str().as_bytes())
                        .map_err(|e| e.to_string())?,
                    &url,
                );
                if prefer_admin || api_key.is_none() {
                    req = req.header(header::AUTHORIZATION, format!("Bearer {admin_token}"));
                } else if let Some(key) = api_key {
                    req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
                }
                if let Some(b) = body {
                    req = req.json(&b);
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| format!("HTTP request to {url} failed: {e}"))?;
                let status = resp.status();
                let text = resp
                    .text()
                    .await
                    .map_err(|e| format!("failed to read HTTP response body: {e}"))?;
                let parsed: Value = serde_json::from_str(&text)
                    .unwrap_or_else(|_| serde_json::json!({ "raw": text }));
                if !status.is_success() {
                    return Err(format!("HTTP {}: {}", status.as_u16(), parsed));
                }
                Ok(parsed)
            }
            Self::Standalone {
                router,
                admin_token,
                ..
            } => {
                let mut builder = Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"));
                let body_bytes = if let Some(b) = body {
                    builder = builder.header(header::CONTENT_TYPE, "application/json");
                    serde_json::to_vec(&b).map_err(|e| e.to_string())?
                } else {
                    Vec::new()
                };
                let request = builder
                    .body(Body::from(body_bytes))
                    .map_err(|e| e.to_string())?;
                let response = match router.clone().oneshot(request).await {
                    Ok(resp) => resp,
                    Err(never) => match never {},
                };
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
                    .await
                    .map_err(|e| e.to_string())?;
                let parsed: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                    serde_json::json!({ "raw": String::from_utf8_lossy(&bytes) })
                });
                if !status.is_success() {
                    return Err(format!("HTTP {}: {}", status.as_u16(), parsed));
                }
                Ok(parsed)
            }
        }
    }
}
