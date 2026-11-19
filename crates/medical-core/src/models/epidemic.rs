use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
    AllYear,
}

impl Season {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Spring => "spring",
            Self::Summer => "summer",
            Self::Autumn => "autumn",
            Self::Winter => "winter",
            Self::AllYear => "all_year",
        }
    }

    pub fn from_str_checked(s: &str) -> Option<Self> {
        match s {
            "spring" => Some(Self::Spring),
            "summer" => Some(Self::Summer),
            "autumn" => Some(Self::Autumn),
            "winter" => Some(Self::Winter),
            "all_year" => Some(Self::AllYear),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpidemicContext {
    pub context_id: String,
    pub region_code: String,
    pub region_name: String,
    pub season: Season,
    pub disease_name: String,
    pub seasonal_tag: String,
    pub intensity: f32,
    #[serde(default)]
    pub boost_symptoms: Vec<String>,
    #[serde(default)]
    pub priority_question_hint: String,
    pub active: bool,
    pub valid_from: NaiveDate,
    pub valid_to: NaiveDate,
}
