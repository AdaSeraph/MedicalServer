pub mod clinical;
pub mod dictionary;
pub mod epidemic;
pub mod knowledge;
pub mod patient;
pub mod triage;

pub use clinical::{
    AbnormalFlag, ClinicalDataCatalog, ClinicalExamBundle, ClinicalMediaAsset,
    ClinicalMediaCategory, ClinicalTypeDescriptor, ImagingExamRecord, ImagingFeatureRef,
    ImagingModality, LabBiochemItem, LabBiochemPanel, LabPanelCategory,
    StandardLabAnalyteTemplate, TriggerCategory, TriggerSeverity, TriggerSubstanceRecord,
    VitalSignsRecord, WoundAssessmentRecord, WoundFeatureRef, default_clinical_data_catalog,
};
pub use dictionary::{MedicalDictionaries, RedFlagLevel, RedFlagRule, SymptomSynonym};
pub use epidemic::{EpidemicContext, Season};
pub use knowledge::{
    KnowledgeDocument, SectionType, SourceType, SuggestedFollowUpSlot, TargetPopulation,
};
pub use patient::{
    Gender, L2Profile, L2SensitivePayload, L3SensitivePayload, L3VisitEvent, PatientRecord,
};
pub use triage::{
    ActiveEpidemicSummary, Citation, ConversationTurn, RecalledL3Event, TriageResultCard,
    TriageStatus,
};

use serde::{Deserialize, Serialize};

fn default_schema_version() -> String {
    "1.0.0".to_string()
}

/// Unified data collection package aligned with `doc/schemas/data-collection-schema.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DataCollectionPackage {
    #[serde(rename = "$schema_version", default = "default_schema_version")]
    pub schema_version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub knowledge_documents: Vec<KnowledgeDocument>,
    #[serde(default)]
    pub patient_records: Vec<PatientRecord>,
    #[serde(default)]
    pub epidemic_contexts: Vec<EpidemicContext>,
    #[serde(default)]
    pub medical_dictionaries: Option<MedicalDictionaries>,
}
