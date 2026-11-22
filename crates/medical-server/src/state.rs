use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use chrono::NaiveDate;
use medical_core::crypto::FieldCipher;
use medical_core::ecs::{
    CompiledMedicalDictionary, InfraClients, TriageEngine, TriageEngineConfig, TriageResources,
};
use medical_core::llm::{
    EmbeddingProvider, HttpRerankProvider, LlmProvider, MockEmbeddingProvider, MockLlmProvider,
    MockRerankProvider, ModelRouter, ModelTier, OpenAiCompatibleEmbeddingProvider, ProviderConfig,
    ProviderKind, RerankProvider, RouterConfig,
};
use medical_core::models::{
    EpidemicContext, KnowledgeDocument, Season, SectionType, SourceType, TargetPopulation,
};
use medical_core::storage::{
    CoreStores, IssueApiKeyRequest, PgApiKeyStore, PgAuditLogStore, PgCheckpointStore,
    PgDictionaryStore, PgEpidemicStore, PgLlmProviderStore, PgPatientMemoryStore,
    QdrantKnowledgeStore, QdrantPatientL3Store, RedisSessionCache, connect_pg_lazy,
    qdrant::connect_qdrant,
};
use medical_pipeline::{
    DualWriteIndexer, HybridKnowledgeSearchEngine, TantivyKnowledgeIndex,
    parse_collection_bundle_file,
};
use serde::Deserialize;
use tracing::warn;

use crate::config::ServerConfig;
use crate::metrics::ServerMetrics;
use crate::rate_limit::TenantRateLimiter;

#[derive(Debug, Deserialize)]
struct TenantSeedFile {
    #[serde(default)]
    tenants: HashMap<String, TenantSeedEntry>,
    #[serde(default)]
    seed_knowledge: Vec<SeedKnowledgeEntry>,
}

#[derive(Debug, Deserialize)]
struct TenantSeedEntry {
    tenant_id: String,
    name: String,
    api_key: String,
    namespace: String,
    qpm: u32,
    tpm: u32,
    #[serde(default)]
    revoked: bool,
}

#[derive(Debug, Deserialize)]
struct SeedKnowledgeEntry {
    #[serde(default = "default_ns")]
    tenant_namespace: String,
    #[serde(flatten)]
    doc: KnowledgeDocument,
}

fn default_ns() -> String {
    "default".to_string()
}

pub struct AppState {
    pub config: ServerConfig,
    pub stores: CoreStores,
    pub cipher: Arc<FieldCipher>,
    pub embedder: Arc<dyn EmbeddingProvider>,
    pub reranker: Arc<dyn RerankProvider>,
    pub tantivy: Arc<TantivyKnowledgeIndex>,
    pub indexer: Arc<DualWriteIndexer>,
    pub hybrid_search: Arc<HybridKnowledgeSearchEngine>,
    pub model_router: Arc<ModelRouter>,
    pub triage_engine: Arc<TriageEngine>,
    pub rate_limiter: Arc<TenantRateLimiter>,
    pub metrics: Arc<ServerMetrics>,
    pub http_client: reqwest::Client,
}

impl AppState {
    /// Builds an `AppState` sharing already-initialized live stores & engines (used by embedded
    /// all-in-one desktop mode in `medical-studio` so UI edits take effect immediately on HTTP APIs).
    #[allow(clippy::too_many_arguments)]
    pub fn from_shared(
        config: ServerConfig,
        stores: CoreStores,
        cipher: Arc<FieldCipher>,
        embedder: Arc<dyn EmbeddingProvider>,
        reranker: Arc<dyn RerankProvider>,
        tantivy: Arc<TantivyKnowledgeIndex>,
        indexer: Arc<DualWriteIndexer>,
        hybrid_search: Arc<HybridKnowledgeSearchEngine>,
        model_router: Arc<ModelRouter>,
        triage_engine: Arc<TriageEngine>,
        http_client: reqwest::Client,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            stores,
            cipher,
            embedder,
            reranker,
            tantivy,
            indexer,
            hybrid_search,
            model_router,
            triage_engine,
            rate_limiter: Arc::new(TenantRateLimiter::new()),
            metrics: Arc::new(ServerMetrics::new()),
            http_client,
        })
    }

    /// Builds `AppState` from `ServerConfig`, supporting both production clients and R5 offline mode.
    pub async fn build(config: ServerConfig) -> Result<Arc<Self>, String> {
        config.validate_production_security()?;

        let cipher = Arc::new(match FieldCipher::from_hex_or_base64(1, &config.field_encryption_key) {
            Ok(c) => c,
            Err(err) => {
                if !config.offline_mode {
                    return Err(format!(
                        "invalid FIELD_ENCRYPTION_KEY in production mode (refusing silent dev fallback): {err}"
                    ));
                }
                warn!(error = %err, "offline mode: deriving dev FieldCipher from passphrase");
                FieldCipher::from_passphrase_for_dev(&config.field_encryption_key)
            }
        });

        let mut stores = CoreStores::in_memory(cipher.clone());

        // Wire production PG / Redis / Qdrant adapters when offline_mode is disabled
        if !config.offline_mode {
            if let Some(ref db_url) = config.database_url {
                let pool = connect_pg_lazy(db_url, 32)
                    .map_err(|e| format!("failed to init postgres pool: {e}"))?;
                stores.patient_memory =
                    Arc::new(PgPatientMemoryStore::new(pool.clone(), cipher.clone()));
                stores.epidemics = Arc::new(PgEpidemicStore::new(pool.clone()));
                stores.dictionaries = Arc::new(PgDictionaryStore::new(pool.clone()));
                stores.audit_logs = Arc::new(PgAuditLogStore::new(pool.clone()));
                stores.api_keys = Arc::new(PgApiKeyStore::new(pool.clone()));
                stores.llm_providers =
                    Arc::new(PgLlmProviderStore::new(pool.clone(), cipher.clone()));
                stores.checkpoints = Arc::new(PgCheckpointStore::new(pool));
            }
            if let Some(ref redis_url) = config.redis_url {
                let cache = RedisSessionCache::from_url(redis_url)
                    .map_err(|e| format!("failed to init redis session cache: {e}"))?;
                stores.session_cache = Arc::new(cache);
            }
            if let Some(ref qdrant_url) = config.qdrant_url {
                let qclient = connect_qdrant(qdrant_url, None)
                    .map_err(|e| format!("failed to init qdrant client: {e}"))?;
                stores.knowledge_vectors = Arc::new(QdrantKnowledgeStore::new(qclient.clone()));
                stores.patient_l3_vectors = Arc::new(QdrantPatientL3Store::new(qclient));
            }
        }

        let embedder: Arc<dyn EmbeddingProvider> = if !config.offline_mode
            && let Some(ref emb_url) = config.embedding_base_url
        {
            Arc::new(
                OpenAiCompatibleEmbeddingProvider::new(
                    "prod_embedding",
                    emb_url,
                    &config.openai_api_key,
                    &config.embedding_model,
                    1024,
                    8_000,
                )
                .map_err(|e| format!("failed to init embedding provider: {e}"))?,
            )
        } else {
            Arc::new(MockEmbeddingProvider::new(1024))
        };

        let reranker: Arc<dyn RerankProvider> = if !config.offline_mode
            && let Some(ref rerank_url) = config.rerank_base_url
        {
            Arc::new(
                HttpRerankProvider::new(
                    "prod_rerank",
                    rerank_url,
                    &config.openai_api_key,
                    &config.rerank_model,
                    6_000,
                )
                .map_err(|e| format!("failed to init rerank provider: {e}"))?,
            )
        } else {
            Arc::new(MockRerankProvider::new())
        };

        let tantivy = Arc::new(if !config.offline_mode {
            let dir = config
                .tantivy_index_dir
                .clone()
                .unwrap_or_else(|| Path::new("data/tantivy_index").to_path_buf());
            TantivyKnowledgeIndex::open_or_create(&dir)
                .map_err(|e| format!("failed to init persistent tantivy index: {e}"))?
        } else if let Some(ref dir) = config.tantivy_index_dir {
            TantivyKnowledgeIndex::open_or_create(dir)
                .map_err(|e| format!("failed to init persistent tantivy index: {e}"))?
        } else {
            TantivyKnowledgeIndex::in_memory()
                .map_err(|e| format!("failed to init tantivy index: {e}"))?
        });
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

        // Build LLM ModelRouter
        let model_router = Arc::new(build_model_router(&config, &stores).await?);

        // Seed default epidemic context + default medical knowledge in offline mode
        // (in production mode, only seed explicitly configured seed_corpus_file / tenant_keys_file).
        if config.offline_mode {
            seed_default_epidemics(&stores).await;
            seed_default_knowledge(&indexer).await;

            let candidate_bundles = [
                config.seed_corpus_file.clone(),
                Some(Path::new("doc/schemas/data-collection-schema.json").to_path_buf()),
                Some(Path::new("e2e/fixtures/knowledge/rag_corpus.json").to_path_buf()),
            ];
            for path in candidate_bundles.into_iter().flatten() {
                if path.is_file() {
                    seed_from_json_file(&path, &indexer, &stores).await;
                }
            }

            let tenant_file = config.tenant_keys_file.clone().or_else(|| {
                let fallback = Path::new("e2e/fixtures/payloads/tenant_keys.json");
                fallback.is_file().then(|| fallback.to_path_buf())
            });
            if let Some(tf) = tenant_file
                && tf.is_file()
            {
                seed_tenants_file(&tf, &stores, &indexer).await;
            }
        } else {
            if let Some(ref path) = config.seed_corpus_file
                && path.is_file()
            {
                seed_from_json_file(path, &indexer, &stores).await;
            }
            if let Some(ref tf) = config.tenant_keys_file
                && tf.is_file()
            {
                seed_tenants_file(tf, &stores, &indexer).await;
            }
        }

        // Compile initial ECS TriageResources
        let dictionaries = stores
            .dictionaries
            .load_dictionaries()
            .await
            .map_err(|e| format!("load dictionaries: {e}"))?;
        let compiled_dict = Arc::new(
            CompiledMedicalDictionary::compile(dictionaries)
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
            config: TriageEngineConfig::default(),
        };
        let triage_engine = Arc::new(TriageEngine::new(Arc::new(ArcSwap::from_pointee(resources))));

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| format!("build http client: {e}"))?;

        Ok(Arc::new(Self {
            config,
            stores,
            cipher,
            embedder,
            reranker,
            tantivy,
            indexer,
            hybrid_search,
            model_router,
            triage_engine,
            rate_limiter: Arc::new(TenantRateLimiter::new()),
            metrics: Arc::new(ServerMetrics::new()),
            http_client,
        }))
    }

    /// Atomically hot-reloads epidemics, medical dictionaries, and/or LLM router into `TriageEngine`.
    pub async fn reload_hot_resources(
        &self,
        reload_epidemics: bool,
        reload_dictionaries: bool,
        reload_router: bool,
    ) -> Result<(), String> {
        let current = self.triage_engine.resources();

        let next_dict = if reload_dictionaries {
            let raw = self
                .stores
                .dictionaries
                .load_dictionaries()
                .await
                .map_err(|e| format!("reload dictionaries: {e}"))?;
            Arc::new(
                CompiledMedicalDictionary::compile(raw)
                    .map_err(|e| format!("compile dictionaries: {e}"))?,
            )
        } else {
            current.dictionary.clone()
        };

        let next_epidemics = if reload_epidemics {
            Arc::new(
                self.stores
                    .epidemics
                    .list_all()
                    .await
                    .map_err(|e| format!("reload epidemics: {e}"))?,
            )
        } else {
            current.epidemics.clone()
        };

        if reload_router {
            let providers = self
                .stores
                .llm_providers
                .list_providers()
                .await
                .map_err(|e| format!("reload providers: {e}"))?;
            if !providers.is_empty() {
                let router_cfg = self
                    .stores
                    .llm_providers
                    .get_router_config()
                    .await
                    .map_err(|e| format!("reload router config: {e}"))?;
                self.model_router
                    .reload(providers, router_cfg)
                    .map_err(|e| format!("apply router reload: {e}"))?;
            }
        }

        let next = TriageResources {
            dictionary: next_dict,
            epidemics: next_epidemics,
            clients: current.clients.clone(),
            model_router: self.model_router.clone(),
            config: current.config.clone(),
        };
        self.triage_engine.reload_resources(next);
        Ok(())
    }
}

async fn build_model_router(
    config: &ServerConfig,
    stores: &CoreStores,
) -> Result<ModelRouter, String> {
    if let Some(base_url) = &config.openai_base_url {
        let fast_cfg = ProviderConfig {
            id: "openai_fast".to_string(),
            name: "OpenAI Compatible Fast".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: base_url.clone(),
            api_key: config.openai_api_key.clone(),
            model: config.openai_fast_model.clone(),
            tier: ModelTier::Fast,
            timeout_ms: 8000,
            is_intranet: true,
            enabled: true,
            priority: 1,
        };
        let reasoning_cfg = ProviderConfig {
            id: "openai_reasoning".to_string(),
            name: "OpenAI Compatible Reasoning".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: base_url.clone(),
            api_key: config.openai_api_key.clone(),
            model: config.openai_reasoning_model.clone(),
            tier: ModelTier::Reasoning,
            timeout_ms: 12000,
            is_intranet: true,
            enabled: true,
            priority: 1,
        };
        let router_cfg = RouterConfig {
            fast_chain: vec![fast_cfg.id.clone()],
            reasoning_chain: vec![reasoning_cfg.id.clone()],
            default_timeout_ms: 8000,
            circuit_breaker_failure_threshold: 3,
            circuit_breaker_cooldown_secs: 30,
        };
        let _ = stores.llm_providers.upsert_provider(&fast_cfg).await;
        let _ = stores.llm_providers.upsert_provider(&reasoning_cfg).await;
        let _ = stores.llm_providers.save_router_config(&router_cfg).await;
        return ModelRouter::from_configs(vec![fast_cfg, reasoning_cfg], router_cfg)
            .map_err(|e| format!("init openai router: {e}"));
    }

    // Pure offline mock LLM providers (no external HTTP server required)
    let fast = Arc::new(
        MockLlmProvider::new("mock_fast", "mock-fast-v1", ModelTier::Fast).with_default_json(
            r#"{"rewritten_query":"发热 咳嗽 咽痛 呼吸内科","extracted_symptoms":["发热","咳嗽","咽痛"]}"#,
        ),
    );
    let reasoning = Arc::new(
        MockLlmProvider::new("mock_reasoning", "mock-reasoning-v1", ModelTier::Reasoning)
            .with_default_json(
                r#"{"action":"complete","primary_department":"呼吸内科","secondary_departments":["发热门诊"],"urgency_level":2,"summary":"结合当前症状与医学知识库，建议前往呼吸内科或发热门诊进一步评估。"}"#,
            ),
    );
    let router_cfg = RouterConfig {
        fast_chain: vec!["mock_fast".to_string()],
        reasoning_chain: vec!["mock_reasoning".to_string()],
        default_timeout_ms: 5000,
        circuit_breaker_failure_threshold: 3,
        circuit_breaker_cooldown_secs: 30,
    };
    Ok(ModelRouter::new(
        vec![fast as Arc<dyn LlmProvider>, reasoning as Arc<dyn LlmProvider>],
        router_cfg,
    ))
}

async fn seed_default_epidemics(stores: &CoreStores) {
    let default_epi = EpidemicContext {
        context_id: "epi_cn_bj_winter_flu".to_string(),
        region_code: "CN-110000".to_string(),
        region_name: "北京市".to_string(),
        season: Season::Winter,
        disease_name: "甲型流行性感冒".to_string(),
        seasonal_tag: "influenza".to_string(),
        intensity: 0.85,
        boost_symptoms: vec![
            "高热".to_string(),
            "肌肉酸痛".to_string(),
            "咽痛".to_string(),
            "干咳".to_string(),
        ],
        priority_question_hint: "近期身边有家人或同事发烧吗？".to_string(),
        active: true,
        valid_from: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
        valid_to: NaiveDate::from_ymd_opt(2030, 12, 31).unwrap(),
    };
    let _ = stores.epidemics.upsert(&default_epi).await;
}

async fn seed_default_knowledge(indexer: &DualWriteIndexer) {
    let docs = vec![
        KnowledgeDocument {
            doc_id: "seed_flu_internal_01".to_string(),
            source_title: "内科学（第9版）".to_string(),
            source_type: SourceType::Textbook,
            authority_level: 1.0,
            disease_name: "流行性感冒".to_string(),
            aliases: vec!["甲流".to_string(), "流感".to_string()],
            department: "呼吸内科".to_string(),
            secondary_departments: vec!["发热门诊".to_string()],
            target_population: vec![
                TargetPopulation::Adult,
                TargetPopulation::Child,
                TargetPopulation::Elderly,
            ],
            section_type: SectionType::ClinicalManifestation,
            content: "冬春季高发，突发高热、全身肌肉酸痛、咽痛、干咳、乏力，建议呼吸内科或发热门诊评估。".to_string(),
            key_symptoms: vec![
                "高热".to_string(),
                "发热".to_string(),
                "肌肉酸痛".to_string(),
                "咽痛".to_string(),
                "干咳".to_string(),
                "咳嗽".to_string(),
            ],
            differential_points: vec!["普通感冒全身症状较轻".to_string()],
            red_flag_signs: vec!["呼吸困难".to_string(), "持续高热超3天".to_string()],
            seasonal_tags: vec!["influenza".to_string(), "winter".to_string()],
            suggested_follow_up_slots: vec![],
            updated_at: None,
        },
        KnowledgeDocument {
            doc_id: "seed_gastro_01".to_string(),
            source_title: "内科学（第9版）".to_string(),
            source_type: SourceType::Textbook,
            authority_level: 0.95,
            disease_name: "急性胃肠炎".to_string(),
            aliases: vec!["肠胃炎".to_string()],
            department: "消化内科".to_string(),
            secondary_departments: vec!["肠道门诊".to_string()],
            target_population: vec![TargetPopulation::Adult, TargetPopulation::Child],
            section_type: SectionType::ClinicalManifestation,
            content: "急性起病，恶心呕吐、腹痛、腹泻、水样便，常有不洁饮食史，建议消化内科或肠道门诊就诊。".to_string(),
            key_symptoms: vec![
                "腹痛".to_string(),
                "腹泻".to_string(),
                "水样便".to_string(),
                "恶心".to_string(),
                "呕吐".to_string(),
            ],
            differential_points: vec![],
            red_flag_signs: vec!["血便".to_string(), "严重脱水".to_string()],
            seasonal_tags: vec!["gastroenteritis".to_string()],
            suggested_follow_up_slots: vec![],
            updated_at: None,
        },
        KnowledgeDocument {
            doc_id: "seed_pediatric_cough_01".to_string(),
            source_title: "儿科学（第9版）".to_string(),
            source_type: SourceType::Textbook,
            authority_level: 0.95,
            disease_name: "小儿支原体肺炎".to_string(),
            aliases: vec!["支原体".to_string()],
            department: "儿科".to_string(),
            secondary_departments: vec!["呼吸内科".to_string()],
            target_population: vec![TargetPopulation::Child],
            section_type: SectionType::ClinicalManifestation,
            content: "儿童发热伴刺激性干咳，夜间加重，建议优先前往儿科评估。".to_string(),
            key_symptoms: vec!["发热".to_string(), "咳嗽".to_string(), "干咳".to_string()],
            differential_points: vec![],
            red_flag_signs: vec!["呼吸急促".to_string(), "口唇发绀".to_string()],
            seasonal_tags: vec!["mycoplasma".to_string(), "winter".to_string()],
            suggested_follow_up_slots: vec![],
            updated_at: None,
        },
        KnowledgeDocument {
            doc_id: "seed_ortho_lumbar_01".to_string(),
            source_title: "骨科学临床指南".to_string(),
            source_type: SourceType::Guideline,
            authority_level: 0.9,
            disease_name: "腰椎间盘突出症".to_string(),
            aliases: vec![],
            department: "骨科".to_string(),
            secondary_departments: vec!["脊柱外科".to_string()],
            target_population: vec![TargetPopulation::Adult, TargetPopulation::Elderly],
            section_type: SectionType::ClinicalManifestation,
            content: "中老年腰痛伴下肢放射痛或麻木，活动受限，建议骨科就诊。".to_string(),
            key_symptoms: vec!["腰痛".to_string(), "下肢麻木".to_string()],
            differential_points: vec![],
            red_flag_signs: vec!["大小便失禁".to_string()],
            seasonal_tags: vec![],
            suggested_follow_up_slots: vec![],
            updated_at: None,
        },
    ];
    if let Err(e) = indexer.upsert_documents(&docs, "default").await {
        warn!("failed to seed default knowledge: {e}");
    }
}

async fn seed_from_json_file(path: &Path, indexer: &DualWriteIndexer, stores: &CoreStores) {
    if let Ok(pkg) = parse_collection_bundle_file(path) {
        if !pkg.knowledge_documents.is_empty() {
            let _ = indexer
                .upsert_documents(&pkg.knowledge_documents, "default")
                .await;
        }
        for epi in &pkg.epidemic_contexts {
            let _ = stores.epidemics.upsert(epi).await;
        }
        return;
    }

    // Fallback: JSON array of KnowledgeDocuments (e.g. rag_corpus.json)
    if let Ok(raw) = std::fs::read_to_string(path)
        && let Ok(docs) = serde_json::from_str::<Vec<KnowledgeDocument>>(&raw)
        && !docs.is_empty()
    {
        let _ = indexer.upsert_documents(&docs, "default").await;
    }
}

async fn seed_tenants_file(path: &Path, stores: &CoreStores, indexer: &DualWriteIndexer) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(parsed) = serde_json::from_str::<TenantSeedFile>(&raw) else {
        warn!("failed to parse tenant seed file: {}", path.display());
        return;
    };
    for entry in parsed.tenants.values() {
        let req = IssueApiKeyRequest {
            tenant_id: entry.tenant_id.clone(),
            name: entry.name.clone(),
            qpm_limit: entry.qpm,
            tpm_limit: entry.tpm,
            namespace: entry.namespace.clone(),
        };
        let _ = stores
            .api_keys
            .seed_key(&entry.api_key, req, entry.revoked)
            .await;
    }
    for sk in parsed.seed_knowledge {
        let _ = indexer
            .upsert_documents(&[sk.doc], &sk.tenant_namespace)
            .await;
    }
}
