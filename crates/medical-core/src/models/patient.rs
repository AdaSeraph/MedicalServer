use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
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
}

/// Sensitive L2 payload encrypted with AES-256-GCM before persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct L2SensitivePayload {
    pub chronic_diseases: Vec<String>,
    pub allergies: Vec<String>,
    pub surgical_history: Vec<String>,
    pub long_term_medications: Vec<String>,
}

impl From<&L2Profile> for L2SensitivePayload {
    fn from(profile: &L2Profile) -> Self {
        Self {
            chronic_diseases: profile.chronic_diseases.clone(),
            allergies: profile.allergies.clone(),
            surgical_history: profile.surgical_history.clone(),
            long_term_medications: profile.long_term_medications.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L3VisitEvent {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub chief_complaint: String,
    #[serde(default)]
    pub extracted_symptoms: Vec<String>,
    pub recommended_department: String,
    pub urgency_level: u8,
    pub summary: String,
}

/// Sensitive L3 payload encrypted with AES-256-GCM before persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct L3SensitivePayload {
    pub chief_complaint: String,
    pub summary: String,
}

impl From<&L3VisitEvent> for L3SensitivePayload {
    fn from(event: &L3VisitEvent) -> Self {
        Self {
            chief_complaint: event.chief_complaint.clone(),
            summary: event.summary.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatientRecord {
    pub patient_id: String,
    pub consent_long_term_memory: bool,
    pub l2_profile: L2Profile,
    #[serde(default)]
    pub l3_visit_events: Vec<L3VisitEvent>,
}
