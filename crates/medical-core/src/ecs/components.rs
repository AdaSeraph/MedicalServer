use bevy_ecs::prelude::Component;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{
    ActiveEpidemicSummary, Citation, ConversationTurn, L2Profile, RecalledL3Event, RedFlagLevel,
    SourceType, SuggestedFollowUpSlot, TargetPopulation, TriageResultCard, TriageStatus,
};

#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct PatientIdentity {
    pub session_id: String,
    pub patient_id: String,
    pub tenant_id: String,
    pub region_code: Option<String>,
    pub target_population: Vec<TargetPopulation>,
    pub consent_long_term_memory: bool,
}

#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct RawUtterance {
    pub text: String,
    pub scrubbed_text: String,
    pub received_at: DateTime<Utc>,
}

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatientProfile {
    pub loaded: bool,
    pub l2_profile: Option<L2Profile>,
}

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMemory {
    pub turns: Vec<ConversationTurn>,
    pub rewritten_query: Option<String>,
    pub query_rewrite_used_llm: bool,
}

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryRecall {
    pub l3_events: Vec<RecalledL3Event>,
    pub active_epidemics: Vec<ActiveEpidemicSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SynonymMatch {
    pub colloquial: String,
    pub standard: String,
}

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtractedSymptoms {
    pub tokens: Vec<String>,
    pub colloquial_matches: Vec<SynonymMatch>,
    pub normalized_terms: Vec<String>,
    pub normalized_utterance: String,
    pub red_flag_term_hits: Vec<String>,
    pub inferred_duration: Option<String>,
    pub inferred_temperature: Option<String>,
}

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct RedFlagAlert {
    pub level: RedFlagLevel,
    pub matched_rule_id: Option<String>,
    pub matched_terms: Vec<String>,
    pub alert_message: Option<String>,
}

impl RedFlagAlert {
    pub fn is_emergency_120(&self) -> bool {
        self.level == RedFlagLevel::Emergency120
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagEvidenceItem {
    pub doc_id: String,
    pub source_title: String,
    pub source_type: SourceType,
    pub disease_name: String,
    pub department: String,
    pub secondary_departments: Vec<String>,
    pub authority_level: f32,
    pub vector_score: Option<f32>,
    pub bm25_score: Option<f32>,
    pub raw_rerank_score: f32,
    pub seasonal_boost_applied: f32,
    pub authority_multiplier: f32,
    pub final_score: f32,
    pub matched_seasonal_tags: Vec<String>,
    pub snippet: String,
    pub suggested_follow_up_slots: Vec<SuggestedFollowUpSlot>,
}

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagEvidence {
    pub items: Vec<RagEvidenceItem>,
    pub citations: Vec<Citation>,
    pub seasonal_context_applied: Vec<String>,
}

#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct TriageSlots {
    pub chief_complaint: Option<String>,
    pub duration: Option<String>,
    pub temperature_or_vitals: Option<String>,
    pub accompanying_symptoms: Vec<String>,
    pub contact_history: Option<String>,
    pub turn: u8,
    pub max_turns: u8,
    pub next_target_slot: Option<String>,
    pub priority_question_hint_injected: Option<String>,
}

impl Default for TriageSlots {
    fn default() -> Self {
        Self {
            chief_complaint: None,
            duration: None,
            temperature_or_vitals: None,
            accompanying_symptoms: Vec::new(),
            contact_history: None,
            turn: 1,
            max_turns: 4,
            next_target_slot: None,
            priority_question_hint_injected: None,
        }
    }
}

#[derive(Component, Debug, Clone, Serialize, Deserialize)]
pub struct TriageResponse {
    pub status: TriageStatus,
    pub session_id: String,
    pub red_flag: RedFlagLevel,
    pub turn: u8,
    pub max_turns: u8,
    pub question: Option<String>,
    pub quick_options: Vec<String>,
    pub seasonal_context_applied: Vec<String>,
    pub triage_result: Option<TriageResultCard>,
    pub citations: Vec<Citation>,
    pub is_fallback: bool,
}

#[derive(Component, Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsolidationPlan {
    pub should_consolidate: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageTiming {
    pub stage: String,
    pub duration_ms: u64,
    pub skipped: bool,
}

/// Complete whitebox ECS snapshot exposed for `MedicalStudio` Module 5 & audit verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriageTrace {
    pub patient_identity: PatientIdentity,
    pub raw_utterance: RawUtterance,
    pub patient_profile: PatientProfile,
    pub session_memory: SessionMemory,
    pub memory_recall: MemoryRecall,
    pub extracted_symptoms: ExtractedSymptoms,
    pub red_flag_alert: RedFlagAlert,
    pub rag_evidence: RagEvidence,
    pub triage_slots: TriageSlots,
    pub triage_response: TriageResponse,
    pub stage_timings: Vec<StageTiming>,
    pub llm_calls_made: u32,
    pub consolidation_scheduled: bool,
    pub consolidation_completed: bool,
    pub total_duration_ms: u64,
}
