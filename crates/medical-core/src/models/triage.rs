use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::patient::L3VisitEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageStatus {
    FollowUp,
    Completed,
    Emergency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriageResultCard {
    pub primary_department: String,
    #[serde(default)]
    pub secondary_departments: Vec<String>,
    pub urgency_level: u8,
    pub summary: String,
    pub disclaimer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub doc_id: String,
    pub source_title: String,
    pub disease_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveEpidemicSummary {
    pub context_id: String,
    pub disease_name: String,
    pub seasonal_tag: String,
    pub intensity: f32,
    pub boost_symptoms: Vec<String>,
    pub priority_question_hint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecalledL3Event {
    pub event: L3VisitEvent,
    pub similarity_score: f32,
}
