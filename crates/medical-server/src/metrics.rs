use prometheus::{
    Encoder, Histogram, HistogramOpts, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};

pub struct ServerMetrics {
    pub registry: Registry,
    pub requests_total: IntCounterVec,
    pub sse_active_connections: IntGauge,
    pub triage_duration_seconds: Histogram,
    pub llm_errors_total: IntCounter,
    pub rag_duration_seconds: Histogram,
    pub cache_hits_total: IntCounter,
    pub pg_pool_active: IntGauge,
}

impl ServerMetrics {
    pub fn new() -> Self {
        let registry = Registry::new();

        let requests_total = IntCounterVec::new(
            Opts::new(
                "medical_server_requests_total",
                "Total HTTP requests served by medical-server",
            ),
            &["endpoint", "method", "status"],
        )
        .expect("requests_total metric");

        let sse_active_connections = IntGauge::new(
            "medical_server_sse_active_connections",
            "Current active SSE streaming connections",
        )
        .expect("sse_active_connections metric");

        let triage_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "medical_server_triage_duration_seconds",
                "Latency of /v1/triage/chat ECS execution in seconds",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
        )
        .expect("triage_duration_seconds metric");

        let llm_errors_total = IntCounter::new(
            "medical_server_llm_errors_total",
            "Total upstream LLM errors encountered",
        )
        .expect("llm_errors_total metric");

        let rag_duration_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "medical_server_rag_duration_seconds",
                "Latency of hybrid RAG knowledge search in seconds",
            )
            .buckets(vec![0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5]),
        )
        .expect("rag_duration_seconds metric");

        let cache_hits_total = IntCounter::new(
            "medical_server_cache_hits_total",
            "Total semantic / response cache hits",
        )
        .expect("cache_hits_total metric");

        let pg_pool_active = IntGauge::new(
            "medical_server_pg_pool_active",
            "Active PostgreSQL pool connections (0 in offline in-memory mode)",
        )
        .expect("pg_pool_active metric");

        registry
            .register(Box::new(requests_total.clone()))
            .expect("register requests_total");
        registry
            .register(Box::new(sse_active_connections.clone()))
            .expect("register sse_active_connections");
        registry
            .register(Box::new(triage_duration_seconds.clone()))
            .expect("register triage_duration_seconds");
        registry
            .register(Box::new(llm_errors_total.clone()))
            .expect("register llm_errors_total");
        registry
            .register(Box::new(rag_duration_seconds.clone()))
            .expect("register rag_duration_seconds");
        registry
            .register(Box::new(cache_hits_total.clone()))
            .expect("register cache_hits_total");
        registry
            .register(Box::new(pg_pool_active.clone()))
            .expect("register pg_pool_active");

        // Initialize labeled series so /metrics always exports required metric names even on cold start
        requests_total
            .with_label_values(&["/health", "GET", "200"])
            .inc_by(0);

        Self {
            registry,
            requests_total,
            sse_active_connections,
            triage_duration_seconds,
            llm_errors_total,
            rag_duration_seconds,
            cache_hits_total,
            pg_pool_active,
        }
    }

    pub fn render_text(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        let _ = encoder.encode(&metric_families, &mut buffer);
        String::from_utf8(buffer).unwrap_or_default()
    }
}

impl Default for ServerMetrics {
    fn default() -> Self {
        Self::new()
    }
}
