pub mod mock;
pub mod openai;
pub mod router;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub use mock::{MockEmbeddingProvider, MockFailure, MockLlmProvider, MockRerankProvider, MockStep};
pub use openai::{
    HttpRerankProvider, OpenAiCompatibleEmbeddingProvider, OpenAiCompatibleLlmProvider,
};
pub use router::{ModelRouter, ProbeResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTier {
    Fast,
    Reasoning,
}

impl ModelTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Reasoning => "reasoning",
        }
    }

    pub fn from_str_checked(s: &str) -> Option<Self> {
        match s {
            "fast" => Some(Self::Fast),
            "reasoning" => Some(Self::Reasoning),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: MessageRole,
    pub content: String,
}

impl LlmMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    pub messages: Vec<LlmMessage>,
    pub tier: ModelTier,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub response_format_json: bool,
    #[serde(default)]
    pub json_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl LlmRequest {
    pub fn new(tier: ModelTier, messages: Vec<LlmMessage>) -> Self {
        Self {
            messages,
            tier,
            temperature: Some(0.2),
            max_tokens: Some(1024),
            response_format_json: false,
            json_schema: None,
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub provider_id: String,
    pub model: String,
    pub usage: TokenUsage,
    pub latency_ms: u64,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmStreamChunk {
    pub delta: String,
    pub done: bool,
    pub provider_id: String,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderKind {
    #[serde(rename = "openai_compatible")]
    OpenAiCompatible,
    #[serde(rename = "ollama")]
    Ollama,
    #[serde(rename = "vllm")]
    Vllm,
    #[serde(rename = "deepseek")]
    DeepSeek,
    #[serde(rename = "qwen")]
    Qwen,
    #[serde(rename = "gemini_openai")]
    GeminiOpenAi,
    #[serde(rename = "custom_http")]
    CustomHttp,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "openai_compatible",
            Self::Ollama => "ollama",
            Self::Vllm => "vllm",
            Self::DeepSeek => "deepseek",
            Self::Qwen => "qwen",
            Self::GeminiOpenAi => "gemini_openai",
            Self::CustomHttp => "custom_http",
        }
    }

    pub fn from_str_checked(s: &str) -> Option<Self> {
        match s {
            "openai_compatible" | "open_ai_compatible" => Some(Self::OpenAiCompatible),
            "ollama" => Some(Self::Ollama),
            "vllm" => Some(Self::Vllm),
            "deepseek" | "deep_seek" => Some(Self::DeepSeek),
            "qwen" => Some(Self::Qwen),
            "gemini_openai" | "gemini_open_ai" => Some(Self::GeminiOpenAi),
            "custom_http" => Some(Self::CustomHttp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub tier: ModelTier,
    pub timeout_ms: u64,
    pub is_intranet: bool,
    pub enabled: bool,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub fast_chain: Vec<String>,
    pub reasoning_chain: Vec<String>,
    pub default_timeout_ms: u64,
    pub circuit_breaker_failure_threshold: u32,
    pub circuit_breaker_cooldown_secs: u64,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            fast_chain: Vec::new(),
            reasoning_chain: Vec::new(),
            default_timeout_ms: 8000,
            circuit_breaker_failure_threshold: 3,
            circuit_breaker_cooldown_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RerankCandidate {
    pub doc_id: String,
    pub text: String,
    pub initial_score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitBreakerState {
    Closed { consecutive_failures: u32 },
    Open { opened_at_epoch_ms: u64 },
    HalfOpen,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealthMetrics {
    pub provider_id: String,
    pub state: CircuitBreakerState,
    pub total_calls: u64,
    pub total_failures: u64,
    pub rate_limited_count: u64,
    pub avg_latency_ms: f64,
    pub last_ttft_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("provider {provider} rate limited (429)")]
    RateLimited {
        provider: String,
        retry_after_ms: Option<u64>,
    },
    #[error("provider {provider} upstream error {status}: {body}")]
    UpstreamServerError {
        provider: String,
        status: u16,
        body: String,
    },
    #[error("provider {provider} timed out after {timeout_ms}ms")]
    Timeout { provider: String, timeout_ms: u64 },
    #[error("provider {provider} network error: {message}")]
    Network { provider: String, message: String },
    #[error("provider {provider} invalid response: {message}")]
    InvalidResponse { provider: String, message: String },
    #[error("provider {provider} auth failed ({status})")]
    AuthFailed { provider: String, status: u16 },
    #[error("provider {provider} circuit breaker is open")]
    CircuitOpen { provider: String },
    #[error("all providers failed for tier {tier:?}: {errors:?}")]
    AllProvidersFailed {
        tier: ModelTier,
        errors: Vec<String>,
    },
}

pub type LlmResult<T> = Result<T, LlmError>;

#[async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &str;
    fn model_name(&self) -> &str;
    fn tier(&self) -> ModelTier;
    async fn complete_json(&self, req: &LlmRequest) -> LlmResult<LlmResponse>;
    async fn chat_stream(
        &self,
        req: &LlmRequest,
        tx: tokio::sync::mpsc::Sender<LlmStreamChunk>,
    ) -> LlmResult<LlmResponse>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn id(&self) -> &str;
    fn dimension(&self) -> usize;
    async fn embed_batch(&self, texts: &[String]) -> LlmResult<Vec<Vec<f32>>>;

    async fn embed_one(&self, text: &str) -> LlmResult<Vec<f32>> {
        let mut out = self.embed_batch(&[text.to_string()]).await?;
        out.pop().ok_or_else(|| LlmError::InvalidResponse {
            provider: self.id().to_string(),
            message: "empty embedding batch response".to_string(),
        })
    }
}

#[async_trait]
pub trait RerankProvider: Send + Sync {
    fn id(&self) -> &str;
    async fn rerank(&self, query: &str, docs: &[RerankCandidate]) -> LlmResult<Vec<f32>>;
}
