use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::privacy::scrub_pii;

use super::{
    CircuitBreakerState, LlmError, LlmMessage, LlmProvider, LlmRequest, LlmResponse, LlmResult,
    LlmStreamChunk, ModelTier, OpenAiCompatibleLlmProvider, ProviderConfig, ProviderHealthMetrics,
    RouterConfig,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub provider_id: String,
    pub ok: bool,
    pub ttft_ms: Option<u64>,
    pub total_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct ProviderHealthRecord {
    state: CircuitBreakerState,
    opened_at: Option<Instant>,
    consecutive_failures: u32,
    total_calls: u64,
    total_failures: u64,
    rate_limited_count: u64,
    latency_sum_ms: u64,
    last_ttft_ms: Option<u64>,
    last_error: Option<String>,
}

impl Default for ProviderHealthRecord {
    fn default() -> Self {
        Self {
            state: CircuitBreakerState::Closed {
                consecutive_failures: 0,
            },
            opened_at: None,
            consecutive_failures: 0,
            total_calls: 0,
            total_failures: 0,
            rate_limited_count: 0,
            latency_sum_ms: 0,
            last_ttft_ms: None,
            last_error: None,
        }
    }
}

pub struct ModelRouter {
    providers: RwLock<HashMap<String, Arc<dyn LlmProvider>>>,
    router_config: RwLock<RouterConfig>,
    health: DashMap<String, ProviderHealthRecord>,
}

impl ModelRouter {
    pub fn new(
        providers: Vec<Arc<dyn LlmProvider>>,
        router_config: RouterConfig,
    ) -> Self {
        let mut map = HashMap::new();
        let health = DashMap::new();
        for p in providers {
            let id = p.id().to_string();
            health.entry(id.clone()).or_default();
            map.insert(id, p);
        }
        Self {
            providers: RwLock::new(map),
            router_config: RwLock::new(router_config),
            health,
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new(), RouterConfig::default())
    }

    pub fn from_configs(
        configs: Vec<ProviderConfig>,
        router_config: RouterConfig,
    ) -> LlmResult<Self> {
        let mut providers: Vec<Arc<dyn LlmProvider>> = Vec::new();
        for cfg in configs.into_iter().filter(|c| c.enabled) {
            providers.push(Arc::new(OpenAiCompatibleLlmProvider::new(cfg)?));
        }
        Ok(Self::new(providers, router_config))
    }

    pub fn register_provider(&self, provider: Arc<dyn LlmProvider>) {
        let id = provider.id().to_string();
        self.health.entry(id.clone()).or_default();
        self.providers.write().insert(id, provider);
    }

    pub fn reload(
        &self,
        configs: Vec<ProviderConfig>,
        router_config: RouterConfig,
    ) -> LlmResult<()> {
        let mut next_map = HashMap::new();
        for cfg in configs.into_iter().filter(|c| c.enabled) {
            let p = OpenAiCompatibleLlmProvider::new(cfg)?;
            let id = p.id().to_string();
            self.health.entry(id.clone()).or_default();
            next_map.insert(id, Arc::new(p) as Arc<dyn LlmProvider>);
        }
        *self.providers.write() = next_map;
        *self.router_config.write() = router_config;
        Ok(())
    }

    pub fn router_config(&self) -> RouterConfig {
        self.router_config.read().clone()
    }

    pub fn health_metrics(&self) -> Vec<ProviderHealthMetrics> {
        self.health
            .iter()
            .map(|entry| {
                let id = entry.key().clone();
                let rec = entry.value();
                let avg_latency_ms = if rec.total_calls > 0 {
                    rec.latency_sum_ms as f64 / rec.total_calls as f64
                } else {
                    0.0
                };
                ProviderHealthMetrics {
                    provider_id: id,
                    state: rec.state,
                    total_calls: rec.total_calls,
                    total_failures: rec.total_failures,
                    rate_limited_count: rec.rate_limited_count,
                    avg_latency_ms,
                    last_ttft_ms: rec.last_ttft_ms,
                    last_error: rec.last_error.clone(),
                }
            })
            .collect()
    }

    fn scrub_request(req: &LlmRequest) -> LlmRequest {
        let mut cloned = req.clone();
        for msg in &mut cloned.messages {
            msg.content = scrub_pii(&msg.content).scrubbed_text;
        }
        cloned
    }

    fn chain_for_tier(&self, tier: ModelTier) -> Vec<Arc<dyn LlmProvider>> {
        let cfg = self.router_config.read().clone();
        let providers = self.providers.read();
        let configured_chain = match tier {
            ModelTier::Fast => &cfg.fast_chain,
            ModelTier::Reasoning => &cfg.reasoning_chain,
        };
        let mut resolved = Vec::new();
        for id in configured_chain {
            if let Some(p) = providers.get(id) {
                resolved.push(Arc::clone(p));
            }
        }
        if resolved.is_empty() {
            for p in providers.values() {
                if p.tier() == tier {
                    resolved.push(Arc::clone(p));
                }
            }
        }
        resolved
    }

    fn check_and_enter_breaker(&self, provider_id: &str, cooldown_secs: u64) -> Result<bool, LlmError> {
        let mut entry = self.health.entry(provider_id.to_string()).or_default();
        match entry.state {
            CircuitBreakerState::Closed { .. } => Ok(false),
            CircuitBreakerState::HalfOpen => Ok(true),
            CircuitBreakerState::Open { .. } => {
                let elapsed_ok = entry
                    .opened_at
                    .map(|t| t.elapsed() >= Duration::from_secs(cooldown_secs))
                    .unwrap_or(true);
                if elapsed_ok {
                    entry.state = CircuitBreakerState::HalfOpen;
                    Ok(true)
                } else {
                    Err(LlmError::CircuitOpen {
                        provider: provider_id.to_string(),
                    })
                }
            }
        }
    }

    fn record_success(&self, provider_id: &str, latency_ms: u64, ttft_ms: Option<u64>) {
        let mut entry = self.health.entry(provider_id.to_string()).or_default();
        entry.total_calls += 1;
        entry.consecutive_failures = 0;
        entry.opened_at = None;
        entry.state = CircuitBreakerState::Closed {
            consecutive_failures: 0,
        };
        entry.latency_sum_ms += latency_ms;
        if let Some(t) = ttft_ms {
            entry.last_ttft_ms = Some(t);
        }
        entry.last_error = None;
    }

    fn record_failure(
        &self,
        provider_id: &str,
        err: &LlmError,
        was_half_open: bool,
        failure_threshold: u32,
    ) {
        let mut entry = self.health.entry(provider_id.to_string()).or_default();
        entry.total_calls += 1;
        entry.total_failures += 1;
        if matches!(err, LlmError::RateLimited { .. }) {
            entry.rate_limited_count += 1;
        }
        entry.consecutive_failures += 1;
        entry.last_error = Some(err.to_string());

        let threshold = failure_threshold.max(1);
        if was_half_open || entry.consecutive_failures >= threshold {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            entry.opened_at = Some(Instant::now());
            entry.state = CircuitBreakerState::Open {
                opened_at_epoch_ms: now_ms,
            };
        } else {
            entry.state = CircuitBreakerState::Closed {
                consecutive_failures: entry.consecutive_failures,
            };
        }
    }

    pub async fn complete_json(&self, req: &LlmRequest) -> LlmResult<LlmResponse> {
        let sanitized = Self::scrub_request(req);
        let cfg = self.router_config();
        let timeout_ms = req.timeout_ms.unwrap_or(cfg.default_timeout_ms).max(100);
        let chain = self.chain_for_tier(req.tier);

        if chain.is_empty() {
            return Err(LlmError::AllProvidersFailed {
                tier: req.tier,
                errors: vec!["no providers configured for tier".to_string()],
            });
        }

        let mut errors = Vec::new();
        for provider in chain {
            let pid = provider.id().to_string();
            let was_half_open =
                match self.check_and_enter_breaker(&pid, cfg.circuit_breaker_cooldown_secs) {
                    Ok(ho) => ho,
                    Err(e) => {
                        errors.push(e.to_string());
                        continue;
                    }
                };

            let started = Instant::now();
            let call = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                provider.complete_json(&sanitized),
            )
            .await;

            match call {
                Ok(Ok(resp)) => {
                    let lat = started.elapsed().as_millis() as u64;
                    self.record_success(&pid, lat, None);
                    return Ok(resp);
                }
                Ok(Err(err)) => {
                    errors.push(err.to_string());
                    self.record_failure(
                        &pid,
                        &err,
                        was_half_open,
                        cfg.circuit_breaker_failure_threshold,
                    );
                }
                Err(_) => {
                    let err = LlmError::Timeout {
                        provider: pid.clone(),
                        timeout_ms,
                    };
                    errors.push(err.to_string());
                    self.record_failure(
                        &pid,
                        &err,
                        was_half_open,
                        cfg.circuit_breaker_failure_threshold,
                    );
                }
            }
        }

        Err(LlmError::AllProvidersFailed {
            tier: req.tier,
            errors,
        })
    }

    pub async fn chat_stream(
        &self,
        req: &LlmRequest,
        tx: mpsc::Sender<LlmStreamChunk>,
    ) -> LlmResult<LlmResponse> {
        let sanitized = Self::scrub_request(req);
        let cfg = self.router_config();
        let timeout_ms = req.timeout_ms.unwrap_or(cfg.default_timeout_ms).max(100);
        let chain = self.chain_for_tier(req.tier);

        if chain.is_empty() {
            return Err(LlmError::AllProvidersFailed {
                tier: req.tier,
                errors: vec!["no providers configured for tier".to_string()],
            });
        }

        let mut errors = Vec::new();
        for provider in chain {
            let pid = provider.id().to_string();
            let was_half_open =
                match self.check_and_enter_breaker(&pid, cfg.circuit_breaker_cooldown_secs) {
                    Ok(ho) => ho,
                    Err(e) => {
                        errors.push(e.to_string());
                        continue;
                    }
                };

            let (inner_tx, mut inner_rx) = mpsc::channel::<LlmStreamChunk>(64);
            let started = Instant::now();
            let outer_tx = tx.clone();

            let forward_handle = tokio::spawn(async move {
                let mut ttft: Option<u64> = None;
                while let Some(chunk) = inner_rx.recv().await {
                    if ttft.is_none() && !chunk.delta.is_empty() {
                        ttft = Some(started.elapsed().as_millis() as u64);
                    }
                    let _ = outer_tx.send(chunk).await;
                }
                ttft
            });

            let call = tokio::time::timeout(
                Duration::from_millis(timeout_ms),
                provider.chat_stream(&sanitized, inner_tx),
            )
            .await;

            let ttft = forward_handle.await.ok().flatten();

            match call {
                Ok(Ok(resp)) => {
                    let lat = started.elapsed().as_millis() as u64;
                    self.record_success(&pid, lat, ttft);
                    return Ok(resp);
                }
                Ok(Err(err)) => {
                    errors.push(err.to_string());
                    self.record_failure(
                        &pid,
                        &err,
                        was_half_open,
                        cfg.circuit_breaker_failure_threshold,
                    );
                }
                Err(_) => {
                    let err = LlmError::Timeout {
                        provider: pid.clone(),
                        timeout_ms,
                    };
                    errors.push(err.to_string());
                    self.record_failure(
                        &pid,
                        &err,
                        was_half_open,
                        cfg.circuit_breaker_failure_threshold,
                    );
                }
            }
        }

        Err(LlmError::AllProvidersFailed {
            tier: req.tier,
            errors,
        })
    }

    pub async fn probe_provider(&self, config: &ProviderConfig) -> ProbeResult {
        let started = Instant::now();
        let provider = match OpenAiCompatibleLlmProvider::new(config.clone()) {
            Ok(p) => p,
            Err(e) => {
                return ProbeResult {
                    provider_id: config.id.clone(),
                    ok: false,
                    ttft_ms: None,
                    total_ms: started.elapsed().as_millis() as u64,
                    error: Some(e.to_string()),
                };
            }
        };

        let req = LlmRequest {
            messages: vec![LlmMessage::user("请回复 OK")],
            tier: config.tier,
            temperature: Some(0.0),
            max_tokens: Some(8),
            response_format_json: false,
            json_schema: None,
            timeout_ms: Some(config.timeout_ms.max(1000)),
        };

        let (tx, mut rx) = mpsc::channel::<LlmStreamChunk>(16);
        let ttft_task = tokio::spawn(async move {
            let t0 = Instant::now();
            while let Some(c) = rx.recv().await {
                if !c.delta.is_empty() {
                    return Some(t0.elapsed().as_millis() as u64);
                }
            }
            None
        });

        let res = provider.chat_stream(&req, tx).await;
        let ttft_ms = ttft_task.await.ok().flatten();
        let total_ms = started.elapsed().as_millis() as u64;

        match res {
            Ok(_) => ProbeResult {
                provider_id: config.id.clone(),
                ok: true,
                ttft_ms: ttft_ms.or(Some(total_ms)),
                total_ms,
                error: None,
            },
            Err(e) => ProbeResult {
                provider_id: config.id.clone(),
                ok: false,
                ttft_ms,
                total_ms,
                error: Some(e.to_string()),
            },
        }
    }
}
