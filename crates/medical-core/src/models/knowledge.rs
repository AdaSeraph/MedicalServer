use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    Textbook,
    Guideline,
    HospitalManual,
    QaDataset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetPopulation {
    Adult,
    Child,
    Elderly,
    Pregnant,
    Female,
    Male,
}

impl TargetPopulation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Adult => "adult",
            Self::Child => "child",
            Self::Elderly => "elderly",
            Self::Pregnant => "pregnant",
            Self::Female => "female",
            Self::Male => "male",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionType {
    ClinicalManifestation,
    DifferentialDiagnosis,
    RedFlag,
    TriageRule,
}

impl SectionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ClinicalManifestation => "clinical_manifestation",
            Self::DifferentialDiagnosis => "differential_diagnosis",
            Self::RedFlag => "red_flag",
            Self::TriageRule => "triage_rule",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedFollowUpSlot {
    pub slot_key: String,
    pub short_question: String,
    pub quick_options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KnowledgeDocument {
    pub doc_id: String,
    pub source_title: String,
    pub source_type: SourceType,
    pub authority_level: f32,
    pub disease_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub department: String,
    #[serde(default)]
    pub secondary_departments: Vec<String>,
    pub target_population: Vec<TargetPopulation>,
    pub section_type: SectionType,
    pub content: String,
    pub key_symptoms: Vec<String>,
    #[serde(default)]
    pub differential_points: Vec<String>,
    #[serde(default)]
    pub red_flag_signs: Vec<String>,
    #[serde(default)]
    pub seasonal_tags: Vec<String>,
    #[serde(default)]
    pub suggested_follow_up_slots: Vec<SuggestedFollowUpSlot>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}
