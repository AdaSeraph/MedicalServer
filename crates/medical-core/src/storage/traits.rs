use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::llm::{ProviderConfig, RouterConfig};
use crate::models::{
    EpidemicContext, KnowledgeDocument, L2Profile, L3VisitEvent, MedicalDictionaries,
    PatientRecord, RecalledL3Event, RedFlagLevel, RedFlagRule, SectionType, SourceType,
    SuggestedFollowUpSlot, SymptomSynonym, TargetPopulation,
};

use super::StorageResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatientL2Record {
    pub patient_id: String,
    pub consent_long_term_memory: bool,
    pub l2_profile: L2Profile,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WipedMemoryStats {
    pub l2_profiles_deleted: u64,
    pub l3_events_deleted: u64,
    pub qdrant_points_deleted: u64,
    pub l1_sessions_deleted: u64,
}

#[async_trait]
pub trait PatientMemoryStore: Send + Sync {
    async fn get_l2_profile(&self, patient_id: &str) -> StorageResult<Option<PatientL2Record>>;
    async fn upsert_l2_profile(
        &self,
        patient_id: &str,
        consent: bool,
        profile: &L2Profile,
    ) -> StorageResult<()>;
    async fn insert_l3_event(&self, patient_id: &str, event: &L3VisitEvent) -> StorageResult<()>;
    async fn list_l3_events(&self, patient_id: &str, limit: usize) -> StorageResult<Vec<L3VisitEvent>>;
    async fn get_full_memory(
        &self,
        patient_id: &str,
        l3_limit: usize,
    ) -> StorageResult<Option<PatientRecord>>;
    async fn list_patient_ids(&self, limit: usize, offset: usize) -> StorageResult<Vec<String>>;
    async fn delete_patient_memory(&self, patient_id: &str) -> StorageResult<WipedMemoryStats>;
}

#[async_trait]
pub trait PatientL3VectorStore: Send + Sync {
    async fn ensure_collection(&self, vector_dim: u64) -> StorageResult<()>;
    async fn upsert_event_vector(
        &self,
        patient_id: &str,
        event: &L3VisitEvent,
        vector: Vec<f32>,
    ) -> StorageResult<()>;
    async fn recall_similar_events(
        &self,
        patient_id: &str,
        query_vector: &[f32],
        top_k: usize,
    ) -> StorageResult<Vec<RecalledL3Event>>;
    async fn delete_patient_events(&self, patient_id: &str) -> StorageResult<u64>;
}

#[async_trait]
pub trait EpidemicStore: Send + Sync {
    async fn list_active(
        &self,
        region_code: Option<&str>,
        season: Option<&str>,
        as_of: NaiveDate,
    ) -> StorageResult<Vec<EpidemicContext>>;
    async fn list_all(&self) -> StorageResult<Vec<EpidemicContext>>;
    async fn upsert(&self, ctx: &EpidemicContext) -> StorageResult<()>;
    async fn delete(&self, context_id: &str) -> StorageResult<bool>;
}

#[async_trait]
pub trait DictionaryStore: Send + Sync {
    async fn load_dictionaries(&self) -> StorageResult<MedicalDictionaries>;
    async fn list_symptom_synonyms(&self) -> StorageResult<Vec<SymptomSynonym>>;
    async fn save_symptom_synonym(&self, synonym: &SymptomSynonym) -> StorageResult<()>;
    async fn delete_symptom_synonym(&self, standard_term: &str) -> StorageResult<bool>;
    async fn list_red_flag_rules(&self) -> StorageResult<Vec<RedFlagRule>>;
    async fn save_red_flag_rule(&self, rule: &RedFlagRule) -> StorageResult<()>;
    async fn delete_red_flag_rule(&self, rule_id: &str) -> StorageResult<bool>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageAuditLog {
    pub id: Uuid,
    pub session_id: String,
    pub patient_id: String,
    pub tenant_id: Option<String>,
    pub red_flag: RedFlagLevel,
    pub triage_status: String,
    pub recommended_department: Option<String>,
    pub urgency_level: Option<u8>,
    pub provider_id: Option<String>,
    pub model_name: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub latency_ms: u64,
    #[serde(default)]
    pub seasonal_contexts_applied: Vec<String>,
    #[serde(default)]
    pub citation_doc_ids: Vec<String>,
    pub error_message: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderUsageStat {
    pub provider_id: String,
    pub requests: u64,
    pub errors: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayMetricsSummary {
    pub total_requests: u64,
    pub total_errors: u64,
    pub error_rate: f64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub avg_latency_ms: f64,
    pub by_provider: Vec<ProviderUsageStat>,
}

#[async_trait]
pub trait AuditLogStore: Send + Sync {
    async fn append(&self, log: &TriageAuditLog) -> StorageResult<()>;
    async fn query_recent(
        &self,
        limit: usize,
        tenant_id: Option<&str>,
    ) -> StorageResult<Vec<TriageAuditLog>>;
    async fn get_metrics_summary(&self, window_hours: u32) -> StorageResult<GatewayMetricsSummary>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueApiKeyRequest {
    pub tenant_id: String,
    pub name: String,
    pub qpm_limit: u32,
    pub tpm_limit: u32,
    pub namespace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    pub key_id: String,
    pub tenant_id: String,
    pub name: String,
    pub key_prefix: String,
    pub qpm_limit: u32,
    pub tpm_limit: u32,
    pub namespace: String,
    pub revoked: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedApiKey {
    pub record: ApiKeyRecord,
    pub raw_key: String,
}

#[async_trait]
pub trait ApiKeyStore: Send + Sync {
    async fn issue_key(&self, req: IssueApiKeyRequest) -> StorageResult<IssuedApiKey>;
    async fn seed_key(
        &self,
        raw_key: &str,
        req: IssueApiKeyRequest,
        revoked: bool,
    ) -> StorageResult<ApiKeyRecord>;
    async fn verify_key(&self, raw_key: &str) -> StorageResult<Option<ApiKeyRecord>>;
    async fn revoke_key(&self, key_id: &str) -> StorageResult<bool>;
    async fn list_keys(&self) -> StorageResult<Vec<ApiKeyRecord>>;
}

#[async_trait]
pub trait LlmProviderStore: Send + Sync {
    async fn list_providers(&self) -> StorageResult<Vec<ProviderConfig>>;
    async fn get_provider(&self, id: &str) -> StorageResult<Option<ProviderConfig>>;
    async fn upsert_provider(&self, config: &ProviderConfig) -> StorageResult<()>;
    async fn delete_provider(&self, id: &str) -> StorageResult<bool>;
    async fn get_router_config(&self) -> StorageResult<RouterConfig>;
    async fn save_router_config(&self, config: &RouterConfig) -> StorageResult<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
}

impl CheckpointStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestionCheckpoint {
    pub job_id: String,
    pub source_path: String,
    pub job_type: String,
    pub status: CheckpointStatus,
    pub total_items: u64,
    pub processed_items: u64,
    pub failed_items: u64,
    pub byte_offset: u64,
    pub last_doc_id: Option<String>,
    pub error_samples: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[async_trait]
pub trait CheckpointStore: Send + Sync {
    async fn get_checkpoint(&self, job_id: &str) -> StorageResult<Option<IngestionCheckpoint>>;
    async fn save_checkpoint(&self, cp: &IngestionCheckpoint) -> StorageResult<()>;
    async fn list_checkpoints(&self, limit: usize) -> StorageResult<Vec<IngestionCheckpoint>>;
    async fn reset_checkpoint(&self, job_id: &str) -> StorageResult<bool>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTurn {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionSlotsSnapshot {
    pub chief_complaint: Option<String>,
    pub duration: Option<String>,
    pub severity_or_temp: Option<String>,
    pub accompanying_symptoms: Vec<String>,
    pub exposure_history: Option<String>,
    pub turn_count: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub patient_id: String,
    pub turns: Vec<SessionTurn>,
    pub slots: SessionSlotsSnapshot,
    pub updated_at: DateTime<Utc>,
}

#[async_trait]
pub trait SessionCache: Send + Sync {
    async fn get_session(&self, session_id: &str) -> StorageResult<Option<SessionState>>;
    async fn save_session(&self, session: &SessionState, ttl_secs: u64) -> StorageResult<()>;
    async fn delete_session(&self, session_id: &str) -> StorageResult<bool>;
    async fn delete_patient_sessions(&self, patient_id: &str) -> StorageResult<u64>;
    async fn get_cached_response(&self, cache_key: &str) -> StorageResult<Option<String>>;
    async fn set_cached_response(
        &self,
        cache_key: &str,
        payload: &str,
        ttl_secs: u64,
    ) -> StorageResult<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSearchQuery {
    pub query_text: String,
    pub query_vector: Option<Vec<f32>>,
    pub keywords: Vec<String>,
    pub target_population: Option<Vec<TargetPopulation>>,
    pub department: Option<String>,
    pub tenant_namespace: Option<String>,
    pub top_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeSearchHit {
    pub doc_id: String,
    pub source_title: String,
    pub source_type: SourceType,
    pub authority_level: f32,
    pub disease_name: String,
    pub department: String,
    pub secondary_departments: Vec<String>,
    pub target_population: Vec<TargetPopulation>,
    pub section_type: SectionType,
    pub content: String,
    pub key_symptoms: Vec<String>,
    pub red_flag_signs: Vec<String>,
    pub seasonal_tags: Vec<String>,
    pub suggested_follow_up_slots: Vec<SuggestedFollowUpSlot>,
    pub vector_score: Option<f32>,
    pub bm25_score: Option<f32>,
    pub rrf_score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_namespace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorizedKnowledgeChunk {
    pub document: KnowledgeDocument,
    pub vector: Vec<f32>,
    pub tenant_namespace: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KnowledgeVectorFilter {
    pub target_population: Option<Vec<TargetPopulation>>,
    pub department: Option<String>,
    pub tenant_namespace: Option<String>,
}

#[async_trait]
pub trait KnowledgeVectorStore: Send + Sync {
    async fn ensure_collection(&self, vector_dim: u64) -> StorageResult<()>;
    async fn upsert_documents(&self, chunks: &[VectorizedKnowledgeChunk]) -> StorageResult<u64>;
    async fn search_vectors(
        &self,
        vector: &[f32],
        filter: &KnowledgeVectorFilter,
        top_k: usize,
    ) -> StorageResult<Vec<KnowledgeSearchHit>>;
    async fn delete_by_doc_ids(&self, doc_ids: &[String]) -> StorageResult<u64>;
}

#[async_trait]
pub trait KnowledgeSearchEngine: Send + Sync {
    async fn search(&self, query: &KnowledgeSearchQuery) -> StorageResult<Vec<KnowledgeSearchHit>>;
}
