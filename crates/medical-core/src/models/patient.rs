use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::clinical::{ClinicalExamBundle, TriggerSubstanceRecord, VitalSignsRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
    Other,
    #[default]
    Unknown,
}

impl Gender {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Male => "male",
            Self::Female => "female",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "male" => Self::Male,
            "female" => Self::Female,
            "other" => Self::Other,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct L2Profile {
    pub age_group: String,
    #[serde(default)]
    pub gender: Gender,
    #[serde(default)]
    pub is_pregnant: bool,
    #[serde(default)]
    pub chronic_diseases: Vec<String>,
    #[serde(default)]
    pub allergies: Vec<String>,
    #[serde(default)]
    pub surgical_history: Vec<String>,
    #[serde(default)]
    pub long_term_medications: Vec<String>,
    /// 基线生命体征（如日常血压、心率、体重/BMI、空腹血糖等）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_vitals: Option<VitalSignsRecord>,
    /// 长期诱因物质/过敏原档案（含诱因物质图片与反应严重程度）
    #[serde(default)]
    pub trigger_substances: Vec<TriggerSubstanceRecord>,
}

/// Sensitive L2 payload encrypted with AES-256-GCM before persistence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct L2SensitivePayload {
    pub chronic_diseases: Vec<String>,
    pub allergies: Vec<String>,
    pub surgical_history: Vec<String>,
    pub long_term_medications: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_vitals: Option<VitalSignsRecord>,
    #[serde(default)]
    pub trigger_substances: Vec<TriggerSubstanceRecord>,
}

impl From<&L2Profile> for L2SensitivePayload {
    fn from(profile: &L2Profile) -> Self {
        Self {
            chronic_diseases: profile.chronic_diseases.clone(),
            allergies: profile.allergies.clone(),
            surgical_history: profile.surgical_history.clone(),
            long_term_medications: profile.long_term_medications.clone(),
            baseline_vitals: profile.baseline_vitals.clone(),
            trigger_substances: profile.trigger_substances.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct L3VisitEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub chief_complaint: String,
    #[serde(default)]
    pub extracted_symptoms: Vec<String>,
    pub recommended_department: String,
    pub urgency_level: u8,
    pub summary: String,
    /// 本次就诊关联的医学检查与多模态数据（血压/生命体征、抽血生化、核磁/CT影像、伤口图片与评估、诱因物质及图片等）
    #[serde(default)]
    pub clinical_exams: ClinicalExamBundle,
}

/// Sensitive L3 payload encrypted with AES-256-GCM before persistence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct L3SensitivePayload {
    pub chief_complaint: String,
    pub summary: String,
    #[serde(default)]
    pub clinical_exams: ClinicalExamBundle,
}

impl From<&L3VisitEvent> for L3SensitivePayload {
    fn from(event: &L3VisitEvent) -> Self {
        Self {
            chief_complaint: event.chief_complaint.clone(),
            summary: event.summary.clone(),
            clinical_exams: event.clinical_exams.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatientRecord {
    pub patient_id: String,
    pub consent_long_term_memory: bool,
    pub l2_profile: L2Profile,
    #[serde(default)]
    pub l3_visit_events: Vec<L3VisitEvent>,
}
