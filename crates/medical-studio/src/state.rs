use std::sync::Arc;

use arc_swap::ArcSwap;
use chrono::{NaiveDate, Utc};
use medical_core::crypto::FieldCipher;
use medical_core::ecs::{
    CompiledMedicalDictionary, InfraClients, TriageEngine, TriageEngineConfig, TriageResources,
};
use medical_core::llm::{
    EmbeddingProvider, LlmProvider, MockEmbeddingProvider, MockLlmProvider, MockRerankProvider,
    ModelRouter, ModelTier, OpenAiCompatibleLlmProvider, ProviderConfig, ProviderKind,
    RerankProvider, RouterConfig,
};
use medical_core::models::{
    EpidemicContext, Gender, L2Profile, L3VisitEvent, Season,
};
use medical_core::storage::{CoreStores, IssueApiKeyRequest};
use medical_pipeline::{
    CheckpointManager, DualWriteIndexer, HybridKnowledgeSearchEngine, TantivyKnowledgeIndex,
};

pub struct StudioState {
    pub stores: CoreStores,
    pub cipher: Arc<FieldCipher>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub reranker: Arc<dyn RerankProvider>,
    pub tantivy: Arc<TantivyKnowledgeIndex>,
    pub indexer: Arc<DualWriteIndexer>,
    pub hybrid_search: Arc<HybridKnowledgeSearchEngine>,
    pub checkpoints: CheckpointManager,
    pub model_router: Arc<ModelRouter>,
    pub triage_engine: Arc<TriageEngine>,
    pub http_client: reqwest::Client,
}

impl StudioState {
    /// Initializes an offline-ready `StudioState` backed by in-memory stores & Tantivy RAM index.
    pub async fn new_offline() -> Result<Self, String> {
        let cipher = Arc::new(FieldCipher::from_passphrase_for_dev("medical-studio-dev-key"));
        let mut stores = CoreStores::in_memory(cipher.clone());

        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbeddingProvider::new(1024));
        let reranker: Arc<dyn RerankProvider> = Arc::new(MockRerankProvider::new());
        let tantivy = Arc::new(
            TantivyKnowledgeIndex::in_memory()
                .map_err(|e| format!("init studio tantivy index: {e}"))?,
        );
        let indexer = Arc::new(
            DualWriteIndexer::new(
                embedder.clone(),
                stores.knowledge_vectors.clone(),
                tantivy.clone(),
            )
            .with_checkpoint_store(stores.checkpoints.clone()),
        );
        let hybrid_search = Arc::new(
            HybridKnowledgeSearchEngine::new(
                embedder.clone(),
                stores.knowledge_vectors.clone(),
                tantivy.clone(),
            )
            .with_reranker(reranker.clone()),
        );
        stores.knowledge_search = hybrid_search.clone();

        // Seed default epidemic context
        let default_epi = EpidemicContext {
            context_id: "epi_studio_bj_winter_flu".to_string(),
            region_code: "CN-110000".to_string(),
            region_name: "北京市".to_string(),
            season: Season::Winter,
            disease_name: "甲型流行性感冒".to_string(),
            seasonal_tag: "influenza".to_string(),
            intensity: 0.85,
            boost_symptoms: vec!["高热".to_string(), "肌肉酸痛".to_string(), "咽痛".to_string()],
            priority_question_hint: "近期身边有家人或同事发烧吗？".to_string(),
            active: true,
            valid_from: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            valid_to: NaiveDate::from_ymd_opt(2030, 12, 31).unwrap(),
        };
        let _ = stores.epidemics.upsert(&default_epi).await;

        // Seed default debug patient for Module 5 ECS Whitebox Debugger
        let demo_pid = "pat_sha256_0000000000000000000000000000000000000000000000000000000000000001";
        let _ = stores
            .patient_memory
            .upsert_l2_profile(
                demo_pid,
                true,
                &L2Profile {
                    age_group: "30-39".to_string(),
                    gender: Gender::Female,
                    is_pregnant: false,
                    chronic_diseases: vec!["轻度哮喘".to_string()],
                    allergies: vec!["青霉素".to_string()],
                    surgical_history: vec![],
                    long_term_medications: vec!["布地奈德吸入剂".to_string()],
                },
            )
            .await;
        let demo_l3_event = L3VisitEvent {
            event_id: "evt_studio_demo_01".to_string(),
            occurred_at: Utc::now(),
            chief_complaint: "夜间干咳胸闷".to_string(),
            extracted_symptoms: vec!["干咳".to_string(), "胸闷".to_string()],
            recommended_department: "呼吸内科".to_string(),
            urgency_level: 2,
            summary: "换季轻度哮喘发作风险，建议呼吸内科评估".to_string(),
        };
        let _ = stores
            .patient_memory
            .insert_l3_event(demo_pid, &demo_l3_event)
            .await;
        if let Ok(vec) = embedder
            .embed_one(&format!(
                "{} {}",
                demo_l3_event.chief_complaint, demo_l3_event.summary
            ))
            .await
        {
            let _ = stores
                .patient_l3_vectors
                .upsert_event_vector(demo_pid, &demo_l3_event, vec)
                .await;
        }

        // Seed default LLM providers & router config for Module 6
        let fast_cfg = ProviderConfig {
            id: "deepseek_fast".to_string(),
            name: "DeepSeek Fast (Cloud)".to_string(),
            kind: ProviderKind::DeepSeek,
            base_url: "https://api.deepseek.com/v1".to_string(),
            api_key: "sk-studio-placeholder".to_string(),
            model: "deepseek-chat".to_string(),
            tier: ModelTier::Fast,
            timeout_ms: 5000,
            is_intranet: false,
            enabled: true,
            priority: 1,
        };
        let reasoning_cfg = ProviderConfig {
            id: "qwen_med_reasoning".to_string(),
            name: "Qwen-Med 72B (Intranet vLLM)".to_string(),
            kind: ProviderKind::Vllm,
            base_url: "http://10.0.1.50:8000/v1".to_string(),
            api_key: "sk-intranet-vllm".to_string(),
            model: "qwen2.5-med-72b".to_string(),
            tier: ModelTier::Reasoning,
            timeout_ms: 8000,
            is_intranet: true,
            enabled: true,
            priority: 1,
        };
        let _ = stores.llm_providers.upsert_provider(&fast_cfg).await;
        let _ = stores.llm_providers.upsert_provider(&reasoning_cfg).await;
        let _ = stores
            .llm_providers
            .save_router_config(&RouterConfig {
                fast_chain: vec![fast_cfg.id.clone()],
                reasoning_chain: vec![reasoning_cfg.id.clone()],
                default_timeout_ms: 8000,
                circuit_breaker_failure_threshold: 3,
                circuit_breaker_cooldown_secs: 30,
            })
            .await;

        // Seed a sample API key for Module 6
        let _ = stores
            .api_keys
            .seed_key(
                "mk_live_studio_default_his",
                IssueApiKeyRequest {
                    tenant_id: "hospital_his".to_string(),
                    name: "院内 HIS 默认接入".to_string(),
                    qpm_limit: 120,
                    tpm_limit: 100_000,
                    namespace: "default".to_string(),
                },
                false,
            )
            .await;

        // Offline mock ModelRouter for Module 5 whitebox ECS simulation
        let mock_fast = Arc::new(
            MockLlmProvider::new("mock_fast", "mock-fast-v1", ModelTier::Fast).with_default_json(
                r#"{"rewritten_query":"发热 咳嗽 咽痛 呼吸内科","extracted_symptoms":["发热","咳嗽","咽痛"]}"#,
            ),
        );
        let mock_reasoning = Arc::new(
            MockLlmProvider::new("mock_reasoning", "mock-reasoning-v1", ModelTier::Reasoning)
                .with_default_json(
                    r#"{"action":"complete","primary_department":"呼吸内科","secondary_departments":["发热门诊"],"urgency_level":2,"summary":"结合当前冬季高发背景与症状表现，建议前往呼吸内科或发热门诊就诊。"}"#,
                ),
        );
        let model_router = Arc::new(ModelRouter::new(
            vec![
                mock_fast as Arc<dyn LlmProvider>,
                mock_reasoning as Arc<dyn LlmProvider>,
            ],
            RouterConfig {
                fast_chain: vec!["mock_fast".to_string()],
                reasoning_chain: vec!["mock_reasoning".to_string()],
                default_timeout_ms: 5000,
                circuit_breaker_failure_threshold: 3,
                circuit_breaker_cooldown_secs: 30,
            },
        ));

        let raw_dicts = stores
            .dictionaries
            .load_dictionaries()
            .await
            .map_err(|e| format!("load dictionaries: {e}"))?;
        let compiled_dict = Arc::new(
            CompiledMedicalDictionary::compile(raw_dicts)
                .map_err(|e| format!("compile dictionaries: {e}"))?,
        );
        let epidemics = Arc::new(
            stores
                .epidemics
                .list_all()
                .await
                .map_err(|e| format!("load epidemics: {e}"))?,
        );

        let resources = TriageResources {
            dictionary: compiled_dict,
            epidemics,
            clients: Arc::new(InfraClients {
                stores: stores.clone(),
                embedding: embedder.clone(),
                rerank: reranker.clone(),
                cipher: cipher.clone(),
            }),
            model_router: model_router.clone(),
            config: TriageEngineConfig {
                await_consolidation: true,
                ..TriageEngineConfig::default()
            },
        };
        let triage_engine = Arc::new(TriageEngine::new(Arc::new(ArcSwap::from_pointee(resources))));
        let checkpoints = CheckpointManager::new(stores.checkpoints.clone());
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;

        Ok(Self {
            stores,
            cipher,
            embedder,
            reranker,
            tantivy,
            indexer,
            hybrid_search,
            checkpoints,
            model_router,
            triage_engine,
            http_client,
        })
    }

    /// Returns true if a provider config is one of the built-in placeholder entries.
    pub fn is_placeholder_provider(cfg: &ProviderConfig) -> bool {
        let key = cfg.api_key.trim();
        key == "sk-studio-placeholder" || key == "sk-intranet-vllm"
    }

    /// Builds a `ModelRouter` from real enabled providers in `stores.llm_providers`,
    /// falling back to the offline mock router when no real provider is configured.
    pub async fn build_active_model_router(&self) -> (Arc<ModelRouter>, bool, Vec<String>) {
        let providers = self
            .stores
            .llm_providers
            .list_providers()
            .await
            .unwrap_or_default();
        let saved_router_cfg = self
            .stores
            .llm_providers
            .get_router_config()
            .await
            .unwrap_or_default();

        let mut real_cfgs: Vec<ProviderConfig> = providers
            .into_iter()
            .filter(|p| p.enabled && !Self::is_placeholder_provider(p))
            .collect();
        real_cfgs.sort_by_key(|p| p.priority);

        if real_cfgs.is_empty() {
            return (
                self.model_router.clone(),
                false,
                vec!["离线 Mock 模型 (mock-fast-v1 / mock-reasoning-v1)".to_string()],
            );
        }

        let mut instances: Vec<Arc<dyn LlmProvider>> = Vec::new();
        let mut active_ids = Vec::new();
        let mut active_labels = Vec::new();

        for cfg in &real_cfgs {
            if let Ok(prov) = OpenAiCompatibleLlmProvider::new(cfg.clone()) {
                instances.push(Arc::new(prov) as Arc<dyn LlmProvider>);
                active_ids.push(cfg.id.clone());
                active_labels.push(format!("{} ({})", cfg.name, cfg.model));
            }
        }

        if instances.is_empty() {
            return (
                self.model_router.clone(),
                false,
                vec!["离线 Mock 模型 (mock-fast-v1 / mock-reasoning-v1)".to_string()],
            );
        }

        let fast_chain: Vec<String> = saved_router_cfg
            .fast_chain
            .iter()
            .filter(|id| active_ids.contains(id))
            .cloned()
            .collect();
        let reasoning_chain: Vec<String> = saved_router_cfg
            .reasoning_chain
            .iter()
            .filter(|id| active_ids.contains(id))
            .cloned()
            .collect();

        let effective_fast = if fast_chain.is_empty() {
            real_cfgs
                .iter()
                .filter(|c| c.tier == ModelTier::Fast)
                .map(|c| c.id.clone())
                .collect::<Vec<_>>()
        } else {
            fast_chain
        };
        let effective_reasoning = if reasoning_chain.is_empty() {
            let r: Vec<String> = real_cfgs
                .iter()
                .filter(|c| c.tier == ModelTier::Reasoning)
                .map(|c| c.id.clone())
                .collect();
            if r.is_empty() {
                active_ids.clone()
            } else {
                r
            }
        } else {
            reasoning_chain
        };

        let router = Arc::new(ModelRouter::new(
            instances,
            RouterConfig {
                fast_chain: if effective_fast.is_empty() {
                    active_ids.clone()
                } else {
                    effective_fast
                },
                reasoning_chain: effective_reasoning,
                default_timeout_ms: saved_router_cfg.default_timeout_ms.max(15_000),
                circuit_breaker_failure_threshold: saved_router_cfg
                    .circuit_breaker_failure_threshold
                    .max(1),
                circuit_breaker_cooldown_secs: saved_router_cfg
                    .circuit_breaker_cooldown_secs
                    .max(5),
            },
        ));
        (router, true, active_labels)
    }

    /// Recompiles dictionaries, epidemics & active LLM router into `TriageEngine` via `ArcSwap`.
    pub async fn sync_ecs_resources(&self) -> Result<(), String> {
        let current = self.triage_engine.resources();
        let raw_dicts = self
            .stores
            .dictionaries
            .load_dictionaries()
            .await
            .map_err(|e| e.to_string())?;
        let compiled_dict = Arc::new(
            CompiledMedicalDictionary::compile(raw_dicts).map_err(|e| e.to_string())?,
        );
        let epidemics = Arc::new(
            self.stores
                .epidemics
                .list_all()
                .await
                .map_err(|e| e.to_string())?,
        );
        let (model_router, _, _) = self.build_active_model_router().await;
        self.triage_engine.reload_resources(TriageResources {
            dictionary: compiled_dict,
            epidemics,
            clients: current.clients.clone(),
            model_router,
            config: current.config.clone(),
        });
        Ok(())
    }
}
