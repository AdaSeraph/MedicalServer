use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::clinical::{
    ClinicalMediaAsset, ImagingFeatureRef, LabBiochemItem, TriggerSubstanceRecord, WoundFeatureRef,
};

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
    /// 医学检查与生化检验参考（血常规、生化、血压、心电等）
    DiagnosticWorkup,
    /// 影像学与伤口/皮损图谱（MRI/CT/X线/超声/伤口照片）
    ImagingAtlas,
    /// 诱因物质与过敏原防治指南（附诱因识别图）
    TriggerPrevention,
}

impl SectionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ClinicalManifestation => "clinical_manifestation",
            Self::DifferentialDiagnosis => "differential_diagnosis",
            Self::RedFlag => "red_flag",
            Self::TriageRule => "triage_rule",
            Self::DiagnosticWorkup => "diagnostic_workup",
            Self::ImagingAtlas => "imaging_atlas",
            Self::TriggerPrevention => "trigger_prevention",
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
    /// 该疾病典型/参考化验与医学生化指标（如 WBC、ALT、cTnI、CRP、HbA1c 等）
    #[serde(default)]
    pub reference_labs: Vec<LabBiochemItem>,
    /// 典型生命体征/血压/血氧参考阈值（如 "收缩压 ≥140 mmHg"、"SpO2 <93%"）
    #[serde(default)]
    pub reference_vitals: Vec<String>,
    /// 典型影像学检查表现（MRI、CT、X线、超声、心电图等）及示例图
    #[serde(default)]
    pub imaging_features: Vec<ImagingFeatureRef>,
    /// 典型伤口/创面/皮损形态特征及图谱
    #[serde(default)]
    pub wound_features: Vec<WoundFeatureRef>,
    /// 常见诱因物质/过敏原及识别图片
    #[serde(default)]
    pub trigger_substances: Vec<TriggerSubstanceRecord>,
    /// 知识库关联多模态医学图片（伤口图、MRI/CT图、化验单示例、诱因物质图等）
    #[serde(default)]
    pub media_assets: Vec<ClinicalMediaAsset>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}
