use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{
    EmbeddingProvider, LlmError, LlmProvider, LlmRequest, LlmResponse, LlmResult, LlmStreamChunk,
    MessageRole, ModelTier, ProviderConfig, RerankCandidate, RerankProvider, TokenUsage,
};

#[derive(Debug, Serialize)]
struct OpenAiMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Debug, Serialize)]
struct OpenAiResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Debug, Serialize)]
struct OpenAiStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct OpenAiChatRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OpenAiStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<OpenAiResponseFormat>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

impl From<OpenAiUsage> for TokenUsage {
    fn from(u: OpenAiUsage) -> Self {
        Self {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: if u.total_tokens > 0 {
                u.total_tokens
            } else {
                u.prompt_tokens + u.completion_tokens
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiChatChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatChoice {
    message: OpenAiChatChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    #[serde(default)]
    choices: Vec<OpenAiChatChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: OpenAiStreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamEvent {
    #[serde(default)]
    choices: Vec<OpenAiStreamChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Clone)]
pub struct OpenAiCompatibleLlmProvider {
    config: ProviderConfig,
    client: reqwest::Client,
}

impl OpenAiCompatibleLlmProvider {
    pub fn new(config: ProviderConfig) -> LlmResult<Self> {
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(32)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| LlmError::Network {
                provider: config.id.clone(),
                message: e.to_string(),
            })?;
        Ok(Self { config, client })
    }

    fn chat_completions_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }

    fn build_payload<'a>(&'a self, req: &'a LlmRequest, stream: bool) -> OpenAiChatRequest<'a> {
        let messages = req
            .messages
            .iter()
            .map(|m| OpenAiMessage {
                role: match m.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                },
                content: m.content.as_str(),
            })
            .collect();
        OpenAiChatRequest {
            model: &self.config.model,
            messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            stream,
            stream_options: if stream {
                Some(OpenAiStreamOptions {
                    include_usage: true,
                })
            } else {
                None
            },
            response_format: if !stream && req.response_format_json {
                Some(OpenAiResponseFormat {
                    kind: "json_object",
                })
            } else {
                None
            },
        }
    }

    async fn check_status(&self, resp: reqwest::Response) -> LlmResult<reqwest::Response> {
        let status = resp.status().as_u16();
        if resp.status().is_success() {
            return Ok(resp);
        }
        if status == 429 {
            let retry_after_ms = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|secs| secs * 1000);
            return Err(LlmError::RateLimited {
                provider: self.config.id.clone(),
                retry_after_ms,
            });
        }
        if status == 401 || status == 403 {
            return Err(LlmError::AuthFailed {
                provider: self.config.id.clone(),
                status,
            });
        }
        let body = resp.text().await.unwrap_or_default();
        Err(LlmError::UpstreamServerError {
            provider: self.config.id.clone(),
            status,
            body,
        })
    }

    fn map_reqwest_err(&self, err: reqwest::Error, timeout_ms: u64) -> LlmError {
        if err.is_timeout() {
            LlmError::Timeout {
                provider: self.config.id.clone(),
                timeout_ms,
            }
        } else {
            LlmError::Network {
                provider: self.config.id.clone(),
                message: err.to_string(),
            }
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleLlmProvider {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn model_name(&self) -> &str {
        &self.config.model
    }

    fn tier(&self) -> ModelTier {
        self.config.tier
    }

    async fn complete_json(&self, req: &LlmRequest) -> LlmResult<LlmResponse> {
        let started = Instant::now();
        let timeout_ms = req.timeout_ms.unwrap_or(self.config.timeout_ms).max(100);
        let payload = self.build_payload(req, false);

        let mut builder = self
            .client
            .post(self.chat_completions_url())
            .timeout(Duration::from_millis(timeout_ms))
            .json(&payload);
        if !self.config.api_key.trim().is_empty() {
            builder = builder.bearer_auth(&self.config.api_key);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| self.map_reqwest_err(e, timeout_ms))?;
        let resp = self.check_status(resp).await?;

        let parsed: OpenAiChatResponse =
            resp.json().await.map_err(|e| LlmError::InvalidResponse {
                provider: self.config.id.clone(),
                message: e.to_string(),
            })?;

        let first = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::InvalidResponse {
                provider: self.config.id.clone(),
                message: "empty choices array".to_string(),
            })?;
        let content = first.message.content.unwrap_or_default();

        Ok(LlmResponse {
            content,
            provider_id: self.config.id.clone(),
            model: self.config.model.clone(),
            usage: parsed.usage.map(Into::into).unwrap_or_default(),
            latency_ms: started.elapsed().as_millis() as u64,
            finish_reason: first.finish_reason,
        })
    }

    async fn chat_stream(
        &self,
        req: &LlmRequest,
        tx: mpsc::Sender<LlmStreamChunk>,
    ) -> LlmResult<LlmResponse> {
        let started = Instant::now();
        let timeout_ms = req.timeout_ms.unwrap_or(self.config.timeout_ms).max(100);
        let payload = self.build_payload(req, true);

        let mut builder = self
            .client
            .post(self.chat_completions_url())
            .timeout(Duration::from_millis(timeout_ms))
            .json(&payload);
        if !self.config.api_key.trim().is_empty() {
            builder = builder.bearer_auth(&self.config.api_key);
        }

        let resp = builder
            .send()
            .await
            .map_err(|e| self.map_reqwest_err(e, timeout_ms))?;
        let resp = self.check_status(resp).await?;

        let mut stream = resp.bytes_stream();
        // Raw byte buffer ensures multi-byte UTF-8 Chinese characters split across TCP chunks
        // are only decoded after a full `\n`-delimited SSE line has arrived.
        let mut raw_buf: Vec<u8> = Vec::with_capacity(4096);
        let mut full_content = String::new();
        let mut usage = TokenUsage::default();
        let mut finish_reason = None;
        let mut done_received = false;

        while let Some(chunk_res) = stream.next().await {
            let bytes = chunk_res.map_err(|e| self.map_reqwest_err(e, timeout_ms))?;
            raw_buf.extend_from_slice(&bytes);

            while let Some(newline_pos) = raw_buf.iter().position(|&b| b == b'\n') {
                let mut line_bytes: Vec<u8> = raw_buf.drain(..=newline_pos).collect();
                if line_bytes.ends_with(b"\n") {
                    line_bytes.pop();
                }
                if line_bytes.ends_with(b"\r") {
                    line_bytes.pop();
                }
                let line = String::from_utf8_lossy(&line_bytes);
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with(':') || trimmed.starts_with("event:") {
                    continue;
                }
                if let Some(data) = trimmed.strip_prefix("data:") {
                    let payload_str = data.trim();
                    if payload_str == "[DONE]" {
                        done_received = true;
                        break;
                    }
                    if let Ok(event) = serde_json::from_str::<OpenAiStreamEvent>(payload_str) {
                        if let Some(u) = event.usage {
                            usage = u.into();
                        }
                        for choice in event.choices {
                            if let Some(fr) = choice.finish_reason {
                                finish_reason = Some(fr);
                            }
                            if let Some(delta) = choice.delta.content
                                && !delta.is_empty()
                            {
                                full_content.push_str(&delta);
                                let _ = tx
                                    .send(LlmStreamChunk {
                                        delta,
                                        done: false,
                                        provider_id: self.config.id.clone(),
                                        usage: None,
                                    })
                                    .await;
                            }
                        }
                    }
                }
            }

            if done_received {
                break;
            }
        }

        // Flush any remaining trailing line without newline
        if !done_received && !raw_buf.is_empty() {
            let line = String::from_utf8_lossy(&raw_buf);
            let trimmed = line.trim();
            if let Some(data) = trimmed.strip_prefix("data:") {
                let payload_str = data.trim();
                if payload_str != "[DONE]"
                    && let Ok(event) = serde_json::from_str::<OpenAiStreamEvent>(payload_str)
                {
                    if let Some(u) = event.usage {
                        usage = u.into();
                    }
                    for choice in event.choices {
                        if let Some(fr) = choice.finish_reason {
                            finish_reason = Some(fr);
                        }
                        if let Some(delta) = choice.delta.content
                            && !delta.is_empty()
                        {
                            full_content.push_str(&delta);
                            let _ = tx
                                .send(LlmStreamChunk {
                                    delta,
                                    done: false,
                                    provider_id: self.config.id.clone(),
                                    usage: None,
                                })
                                .await;
                        }
                    }
                }
            }
        }

        let _ = tx
            .send(LlmStreamChunk {
                delta: String::new(),
                done: true,
                provider_id: self.config.id.clone(),
                usage: Some(usage.clone()),
            })
            .await;

        Ok(LlmResponse {
            content: full_content,
            provider_id: self.config.id.clone(),
            model: self.config.model.clone(),
            usage,
            latency_ms: started.elapsed().as_millis() as u64,
            finish_reason,
        })
    }
}

#[derive(Debug, Serialize)]
struct OpenAiEmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbedItem {
    index: usize,
    embedding: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct OpenAiEmbedResponse {
    data: Vec<OpenAiEmbedItem>,
}

#[derive(Clone)]
pub struct OpenAiCompatibleEmbeddingProvider {
    id: String,
    base_url: String,
    api_key: String,
    model: String,
    dimension: usize,
    timeout_ms: u64,
    client: reqwest::Client,
}

impl OpenAiCompatibleEmbeddingProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        dimension: usize,
        timeout_ms: u64,
    ) -> LlmResult<Self> {
        let id = id.into();
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(16)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| LlmError::Network {
                provider: id.clone(),
                message: e.to_string(),
            })?;
        Ok(Self {
            id,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            dimension,
            timeout_ms,
            client,
        })
    }

    fn embeddings_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/embeddings") {
            base.to_string()
        } else {
            format!("{base}/embeddings")
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleEmbeddingProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed_batch(&self, texts: &[String]) -> LlmResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let payload = OpenAiEmbedRequest {
            model: &self.model,
            input: texts,
        };
        let mut builder = self
            .client
            .post(self.embeddings_url())
            .timeout(Duration::from_millis(self.timeout_ms.max(100)))
            .json(&payload);
        if !self.api_key.trim().is_empty() {
            builder = builder.bearer_auth(&self.api_key);
        }
        let resp = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                LlmError::Timeout {
                    provider: self.id.clone(),
                    timeout_ms: self.timeout_ms,
                }
            } else {
                LlmError::Network {
                    provider: self.id.clone(),
                    message: e.to_string(),
                }
            }
        })?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            if status == 429 {
                return Err(LlmError::RateLimited {
                    provider: self.id.clone(),
                    retry_after_ms: None,
                });
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::UpstreamServerError {
                provider: self.id.clone(),
                status,
                body,
            });
        }
        let mut parsed: OpenAiEmbedResponse =
            resp.json().await.map_err(|e| LlmError::InvalidResponse {
                provider: self.id.clone(),
                message: e.to_string(),
            })?;
        parsed.data.sort_by_key(|item| item.index);
        if parsed.data.len() != texts.len() {
            return Err(LlmError::InvalidResponse {
                provider: self.id.clone(),
                message: format!(
                    "expected {} embeddings, got {}",
                    texts.len(),
                    parsed.data.len()
                ),
            });
        }
        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }
}

#[derive(Debug, Serialize)]
struct RerankApiRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: Vec<&'a str>,
    top_n: usize,
}

#[derive(Debug, Deserialize)]
struct RerankApiItem {
    index: usize,
    relevance_score: f32,
}

#[derive(Debug, Deserialize)]
struct RerankApiResponse {
    results: Vec<RerankApiItem>,
}

#[derive(Clone)]
pub struct HttpRerankProvider {
    id: String,
    base_url: String,
    api_key: String,
    model: String,
    timeout_ms: u64,
    client: reqwest::Client,
}

impl HttpRerankProvider {
    pub fn new(
        id: impl Into<String>,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout_ms: u64,
    ) -> LlmResult<Self> {
        let id = id.into();
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(16)
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| LlmError::Network {
                provider: id.clone(),
                message: e.to_string(),
            })?;
        Ok(Self {
            id,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            timeout_ms,
            client,
        })
    }

    fn rerank_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/rerank") {
            base.to_string()
        } else {
            format!("{base}/rerank")
        }
    }
}

#[async_trait]
impl RerankProvider for HttpRerankProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn rerank(&self, query: &str, docs: &[RerankCandidate]) -> LlmResult<Vec<f32>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let payload = RerankApiRequest {
            model: &self.model,
            query,
            documents: docs.iter().map(|d| d.text.as_str()).collect(),
            top_n: docs.len(),
        };
        let mut builder = self
            .client
            .post(self.rerank_url())
            .timeout(Duration::from_millis(self.timeout_ms.max(100)))
            .json(&payload);
        if !self.api_key.trim().is_empty() {
            builder = builder.bearer_auth(&self.api_key);
        }
        let resp = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                LlmError::Timeout {
                    provider: self.id.clone(),
                    timeout_ms: self.timeout_ms,
                }
            } else {
                LlmError::Network {
                    provider: self.id.clone(),
                    message: e.to_string(),
                }
            }
        })?;
        let status = resp.status().as_u16();
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::UpstreamServerError {
                provider: self.id.clone(),
                status,
                body,
            });
        }
        let parsed: RerankApiResponse =
            resp.json().await.map_err(|e| LlmError::InvalidResponse {
                provider: self.id.clone(),
                message: e.to_string(),
            })?;
        let mut scores: Vec<f32> = docs.iter().map(|d| d.initial_score).collect();
        for item in parsed.results {
            if item.index < scores.len() {
                scores[item.index] = item.relevance_score;
            }
        }
        Ok(scores)
    }
}
