use std::time::Instant;

use dashmap::DashMap;

#[derive(Debug, Clone)]
struct TokenBucket {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last_refill: Instant,
}

impl TokenBucket {
    fn new(per_minute_limit: u32) -> Self {
        let cap = (per_minute_limit.max(1)) as f64;
        Self {
            capacity: cap,
            tokens: cap,
            refill_per_sec: cap / 60.0,
            last_refill: Instant::now(),
        }
    }

    fn sync_limit(&mut self, per_minute_limit: u32) {
        let cap = (per_minute_limit.max(1)) as f64;
        if (self.capacity - cap).abs() > f64::EPSILON {
            self.capacity = cap;
            self.refill_per_sec = cap / 60.0;
            if self.tokens > cap {
                self.tokens = cap;
            }
        }
    }

    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        if elapsed > 0.0 {
            self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
            self.last_refill = now;
        }
    }

    fn try_consume(&mut self, cost: f64) -> Result<(), u64> {
        self.refill();
        if self.tokens >= cost {
            self.tokens -= cost;
            Ok(())
        } else {
            let deficit = cost - self.tokens;
            let retry_secs = (deficit / self.refill_per_sec).ceil().max(1.0) as u64;
            Err(retry_secs)
        }
    }
}

#[derive(Debug, Clone)]
struct TenantBuckets {
    qpm: TokenBucket,
    tpm: TokenBucket,
}

/// Multi-tenant per-API-key QPM (requests/min) and TPM (tokens/min) token-bucket limiter.
pub struct TenantRateLimiter {
    buckets: DashMap<String, TenantBuckets>,
}

impl TenantRateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: DashMap::new(),
        }
    }

    /// Consumes 1 request token from the tenant's QPM bucket and estimated input tokens from TPM.
    pub fn check_and_consume(
        &self,
        key_id: &str,
        qpm_limit: u32,
        tpm_limit: u32,
        estimated_tokens: u32,
    ) -> Result<(), RateLimitRejection> {
        let mut entry = self
            .buckets
            .entry(key_id.to_string())
            .or_insert_with(|| TenantBuckets {
                qpm: TokenBucket::new(qpm_limit),
                tpm: TokenBucket::new(tpm_limit),
            });

        entry.qpm.sync_limit(qpm_limit);
        entry.tpm.sync_limit(tpm_limit);

        if let Err(retry_after_secs) = entry.qpm.try_consume(1.0) {
            return Err(RateLimitRejection {
                kind: "qpm",
                retry_after_secs,
            });
        }
        if estimated_tokens > 0
            && let Err(retry_after_secs) = entry.tpm.try_consume(estimated_tokens as f64)
        {
            // Roll back QPM token so TPM-rejected requests do not burn QPM quota
            entry.qpm.tokens = (entry.qpm.tokens + 1.0).min(entry.qpm.capacity);
            return Err(RateLimitRejection {
                kind: "tpm",
                retry_after_secs,
            });
        }
        Ok(())
    }

    /// Post-call accounting to deduct actual LLM completion tokens from the tenant's TPM bucket.
    pub fn record_actual_tokens(&self, key_id: &str, tpm_limit: u32, tokens: u32) {
        if tokens == 0 {
            return;
        }
        if let Some(mut entry) = self.buckets.get_mut(key_id) {
            entry.tpm.sync_limit(tpm_limit);
            entry.tpm.refill();
            entry.tpm.tokens = (entry.tpm.tokens - tokens as f64).max(0.0);
        }
    }
}

impl Default for TenantRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct RateLimitRejection {
    pub kind: &'static str,
    pub retry_after_secs: u64,
}
