pub mod memory;
pub mod pg;
pub mod qdrant;
pub mod redis_cache;
pub mod traits;

use std::sync::Arc;

use crate::crypto::{CryptoError, FieldCipher};

pub use memory::{
    InMemoryApiKeyStore, InMemoryAuditLogStore, InMemoryCheckpointStore, InMemoryDictionaryStore,
    InMemoryEpidemicStore, InMemoryKnowledgeStore, InMemoryLlmProviderStore,
    InMemoryPatientL3VectorStore, InMemoryPatientMemoryStore, InMemorySessionCache,
};
pub use pg::{
    PgApiKeyStore, PgAuditLogStore, PgCheckpointStore, PgDictionaryStore, PgEpidemicStore,
    PgLlmProviderStore, PgPatientMemoryStore, connect_pg_lazy,
};
pub use qdrant::{QdrantKnowledgeStore, QdrantPatientL3Store};
pub use redis_cache::RedisSessionCache;
pub use traits::{
    ApiKeyRecord, ApiKeyStore, AuditLogStore, CheckpointStatus, CheckpointStore, DictionaryStore,
    EpidemicStore, GatewayMetricsSummary, IngestionCheckpoint, IssueApiKeyRequest, IssuedApiKey,
    KnowledgeSearchEngine, KnowledgeSearchHit, KnowledgeSearchQuery, KnowledgeVectorFilter,
    KnowledgeVectorStore, LlmProviderStore, PatientL2Record, PatientL3VectorStore,
    PatientMemoryStore, ProviderUsageStat, SessionCache, SessionSlotsSnapshot, SessionState,
    SessionTurn, TriageAuditLog, VectorizedKnowledgeChunk, WipedMemoryStats,
};

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{entity} not found: {id}")]
    NotFound { entity: &'static str, id: String },
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("cache error: {0}")]
    Cache(String),
    #[error("vector store error: {0}")]
    VectorStore(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

impl From<CryptoError> for StorageError {
    fn from(err: CryptoError) -> Self {
        Self::Crypto(err.to_string())
    }
}

impl From<sqlx::Error> for StorageError {
    fn from(err: sqlx::Error) -> Self {
        match err {
            sqlx::Error::RowNotFound => Self::NotFound {
                entity: "record",
                id: "unknown".to_string(),
            },
            other => Self::Database(other.to_string()),
        }
    }
}

impl From<redis::RedisError> for StorageError {
    fn from(err: redis::RedisError) -> Self {
        Self::Cache(err.to_string())
    }
}

impl From<qdrant_client::QdrantError> for StorageError {
    fn from(err: qdrant_client::QdrantError) -> Self {
        Self::VectorStore(err.to_string())
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialization(err.to_string())
    }
}

#[derive(Clone)]
pub struct CoreStores {
    pub patient_memory: Arc<dyn PatientMemoryStore>,
    pub patient_l3_vectors: Arc<dyn PatientL3VectorStore>,
    pub epidemics: Arc<dyn EpidemicStore>,
    pub dictionaries: Arc<dyn DictionaryStore>,
    pub audit_logs: Arc<dyn AuditLogStore>,
    pub api_keys: Arc<dyn ApiKeyStore>,
    pub llm_providers: Arc<dyn LlmProviderStore>,
    pub checkpoints: Arc<dyn CheckpointStore>,
    pub session_cache: Arc<dyn SessionCache>,
    pub knowledge_vectors: Arc<dyn KnowledgeVectorStore>,
    pub knowledge_search: Arc<dyn KnowledgeSearchEngine>,
}

impl CoreStores {
    /// R5 offline constructor: every store backed by thread-safe in-memory implementations.
    pub fn in_memory(cipher: Arc<FieldCipher>) -> Self {
        let knowledge = Arc::new(InMemoryKnowledgeStore::new());
        Self {
            patient_memory: Arc::new(InMemoryPatientMemoryStore::new(cipher)),
            patient_l3_vectors: Arc::new(InMemoryPatientL3VectorStore::new()),
            epidemics: Arc::new(InMemoryEpidemicStore::new()),
            dictionaries: Arc::new(InMemoryDictionaryStore::with_defaults()),
            audit_logs: Arc::new(InMemoryAuditLogStore::new()),
            api_keys: Arc::new(InMemoryApiKeyStore::new()),
            llm_providers: Arc::new(InMemoryLlmProviderStore::new()),
            checkpoints: Arc::new(InMemoryCheckpointStore::new()),
            session_cache: Arc::new(InMemorySessionCache::new()),
            knowledge_vectors: knowledge.clone(),
            knowledge_search: knowledge,
        }
    }
}
