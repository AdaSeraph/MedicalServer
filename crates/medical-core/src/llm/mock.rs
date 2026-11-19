use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::mpsc;

use super::{
    EmbeddingProvider, LlmError, LlmProvider, LlmRequest, LlmResponse, LlmResult, LlmStreamChunk,
    ModelTier, RerankCandidate, RerankProvider, TokenUsage,
};

#[derive(Debug, Clone)]
pub enum MockFailure {
    RateLimit429,
    ServerError500,
    Timeout { sleep_ms: u64 },
    NetworkError,
}

#[derive(Debug, Clone)]
pub enum MockStep {
    Json(String),
    StreamTokens(Vec<String>),
    Fail(MockFailure),
}

pub struct MockLlmProvider {
    id: String,
    model: String,
    tier: ModelTier,
    default_json: Mutex<String>,
    steps: Mutex<VecDeque<MockStep>>,
    call_count: AtomicUsize,
    recorded_requests: Mutex<Vec<LlmRequest>>,
}

impl MockLlmProvider {
    pub fn new(id: impl Into<String>, model: impl Into<String>, tier: ModelTier) -> Self {
        Self {
            id: id.into(),
            model: model.into(),
            tier,
            default_json: Mutex::new("{}".to_string()),
            steps: Mutex::new(VecDeque::new()),
            call_count: AtomicUsize::new(0),
            recorded_requests: Mutex::new(Vec::new()),
        }
    }

    pub fn with_default_json(self, json: impl Into<String>) -> Self {
        *self.default_json.lock() = json.into();
        self
    }

    pub fn set_default_json(&self, json: impl Into<String>) {
        *self.default_json.lock() = json.into();
    }

    pub fn push_step(&self, step: MockStep) {
        self.steps.lock().push_back(step);
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    pub fn recorded_requests(&self) -> Vec<LlmRequest> {
        self.recorded_requests.lock().clone()
    }

    async fn map_failure(&self, failure: MockFailure) -> LlmError {
        match failure {
            MockFailure::RateLimit429 => LlmError::RateLimited {
                provider: self.id.clone(),
                retry_after_ms: Some(500),
            },
            MockFailure::ServerError500 => LlmError::UpstreamServerError {
                provider: self.id.clone(),
                status: 503,
                body: "mock upstream unavailable".to_string(),
            },
            MockFailure::Timeout { sleep_ms } => {
                if sleep_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
                }
                LlmError::Timeout {
                    provider: self.id.clone(),
                    timeout_ms: sleep_ms,
                }
            }
            MockFailure::NetworkError => LlmError::Network {
                provider: self.id.clone(),
                message: "mock connection reset".to_string(),
            },
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn tier(&self) -> ModelTier {
        self.tier
    }

    async fn complete_json(&self, req: &LlmRequest) -> LlmResult<LlmResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.recorded_requests.lock().push(req.clone());

        let next = self.steps.lock().pop_front();
        match next {
            Some(MockStep::Fail(f)) => Err(self.map_failure(f).await),
            Some(MockStep::Json(s)) => Ok(LlmResponse {
                content: s,
                provider_id: self.id.clone(),
                model: self.model.clone(),
                usage: TokenUsage {
                    prompt_tokens: 42,
                    completion_tokens: 24,
                    total_tokens: 66,
                },
                latency_ms: 5,
                finish_reason: Some("stop".to_string()),
            }),
            Some(MockStep::StreamTokens(tokens)) => Ok(LlmResponse {
                content: tokens.concat(),
                provider_id: self.id.clone(),
                model: self.model.clone(),
                usage: TokenUsage {
                    prompt_tokens: 42,
                    completion_tokens: 24,
                    total_tokens: 66,
                },
                latency_ms: 5,
                finish_reason: Some("stop".to_string()),
            }),
            None => Ok(LlmResponse {
                content: self.default_json.lock().clone(),
                provider_id: self.id.clone(),
                model: self.model.clone(),
                usage: TokenUsage {
                    prompt_tokens: 40,
                    completion_tokens: 20,
                    total_tokens: 60,
                },
                latency_ms: 4,
                finish_reason: Some("stop".to_string()),
            }),
        }
    }

    async fn chat_stream(
        &self,
        req: &LlmRequest,
        tx: mpsc::Sender<LlmStreamChunk>,
    ) -> LlmResult<LlmResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.recorded_requests.lock().push(req.clone());

        let next = self.steps.lock().pop_front();
        let (tokens, usage) = match next {
            Some(MockStep::Fail(f)) => return Err(self.map_failure(f).await),
            Some(MockStep::StreamTokens(tokens)) => (
                tokens,
                TokenUsage {
                    prompt_tokens: 50,
                    completion_tokens: 30,
                    total_tokens: 80,
                },
            ),
            Some(MockStep::Json(s)) => (
                vec![s],
                TokenUsage {
                    prompt_tokens: 50,
                    completion_tokens: 30,
                    total_tokens: 80,
                },
            ),
            None => (
                vec![self.default_json.lock().clone()],
                TokenUsage {
                    prompt_tokens: 50,
                    completion_tokens: 30,
                    total_tokens: 80,
                },
            ),
        };

        let mut full = String::new();
        for tok in tokens {
            full.push_str(&tok);
            let _ = tx
                .send(LlmStreamChunk {
                    delta: tok,
                    done: false,
                    provider_id: self.id.clone(),
                    usage: None,
                })
                .await;
        }
        let _ = tx
            .send(LlmStreamChunk {
                delta: String::new(),
                done: true,
                provider_id: self.id.clone(),
                usage: Some(usage.clone()),
            })
            .await;

        Ok(LlmResponse {
            content: full,
            provider_id: self.id.clone(),
            model: self.model.clone(),
            usage,
            latency_ms: 5,
            finish_reason: Some("stop".to_string()),
        })
    }
}

pub struct MockEmbeddingProvider {
    id: String,
    dimension: usize,
}

impl MockEmbeddingProvider {
    pub fn new(dimension: usize) -> Self {
        Self {
            id: "mock-embedding".to_string(),
            dimension: dimension.max(8),
        }
    }

    pub fn deterministic_embed(&self, text: &str) -> Vec<f32> {
        let mut vec = vec![0.0_f32; self.dimension];
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            vec[0] = 1.0;
            return vec;
        }
        for c in &chars {
            let idx = (*c as usize).wrapping_mul(2654435761) % self.dimension;
            vec[idx] += 1.0;
        }
        for win in chars.windows(2) {
            let h = ((win[0] as usize).wrapping_mul(131)) ^ (win[1] as usize);
            let idx = h % self.dimension;
            vec[idx] += 2.0;
        }
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        } else {
            vec[0] = 1.0;
        }
        vec
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed_batch(&self, texts: &[String]) -> LlmResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.deterministic_embed(t)).collect())
    }
}

pub struct MockRerankProvider {
    id: String,
    fail_with: Mutex<Option<MockFailure>>,
}

impl Default for MockRerankProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRerankProvider {
    pub fn new() -> Self {
        Self {
            id: "mock-rerank".to_string(),
            fail_with: Mutex::new(None),
        }
    }

    pub fn set_failure(&self, failure: Option<MockFailure>) {
        *self.fail_with.lock() = failure;
    }
}

#[async_trait]
impl RerankProvider for MockRerankProvider {
    fn id(&self) -> &str {
        &self.id
    }

    async fn rerank(&self, query: &str, docs: &[RerankCandidate]) -> LlmResult<Vec<f32>> {
        if let Some(fail) = self.fail_with.lock().clone() {
            return match fail {
                MockFailure::RateLimit429 => Err(LlmError::RateLimited {
                    provider: self.id.clone(),
                    retry_after_ms: None,
                }),
                MockFailure::ServerError500 => Err(LlmError::UpstreamServerError {
                    provider: self.id.clone(),
                    status: 500,
                    body: "mock rerank error".to_string(),
                }),
                MockFailure::Timeout { .. } => Err(LlmError::Timeout {
                    provider: self.id.clone(),
                    timeout_ms: 1000,
                }),
                MockFailure::NetworkError => Err(LlmError::Network {
                    provider: self.id.clone(),
                    message: "mock rerank network error".to_string(),
                }),
            };
        }

        let q_chars: HashSet<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
        let scores = docs
            .iter()
            .map(|d| {
                let overlap = if q_chars.is_empty() {
                    0.0
                } else {
                    let hits = d.text.chars().filter(|c| q_chars.contains(c)).count();
                    (hits as f32 / (q_chars.len() as f32 * 2.0)).min(1.0)
                };
                (0.6 * d.initial_score + 0.4 * overlap).clamp(0.0, 1.0)
            })
            .collect();
        Ok(scores)
    }
}
