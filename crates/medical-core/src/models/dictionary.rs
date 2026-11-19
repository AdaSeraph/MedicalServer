use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymptomSynonym {
    pub standard_term: String,
    pub colloquial_terms: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub enum RedFlagLevel {
    #[default]
    None,
    Urgent,
    Emergency120,
}

impl RedFlagLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Urgent => "Urgent",
            Self::Emergency120 => "Emergency120",
        }
    }

    pub fn from_str_checked(s: &str) -> Option<Self> {
        match s {
            "None" => Some(Self::None),
            "Urgent" => Some(Self::Urgent),
            "Emergency120" => Some(Self::Emergency120),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedFlagRule {
    pub rule_id: String,
    pub level: RedFlagLevel,
    #[serde(default)]
    pub trigger_all_of: Vec<String>,
    #[serde(default)]
    pub trigger_any_of: Vec<String>,
    pub alert_message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MedicalDictionaries {
    #[serde(default)]
    pub symptom_synonyms: Vec<SymptomSynonym>,
    #[serde(default)]
    pub red_flag_rules: Vec<RedFlagRule>,
}
