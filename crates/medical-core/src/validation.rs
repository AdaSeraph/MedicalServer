use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::models::{
    ClinicalExamBundle, ClinicalMediaAsset, DataCollectionPackage, EpidemicContext,
    ImagingExamRecord, KnowledgeDocument, L2Profile, L3VisitEvent, LabBiochemPanel,
    MedicalDictionaries, PatientRecord, RedFlagLevel, RedFlagRule, SymptomSynonym,
    TriggerSubstanceRecord, VitalSignsRecord, WoundAssessmentRecord,
};

static RE_CANONICAL_PATIENT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^pat_sha256_[0-9a-f]{64}$").expect("valid canonical patient id regex"));
static RE_EMBEDDED_ID_CARD_18: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\D)\d{6}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dx](?:$|\D)").expect("valid 18-digit id regex"));
static RE_EMBEDDED_ID_CARD_15: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\D)\d{6}\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}(?:$|\D)").expect("valid 15-digit id regex"));
static RE_ANY_18_OR_15_DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\D)(?:\d{17}[\dx]|\d{15})(?:$|\D)").expect("valid generic id regex"));
static RE_EMBEDDED_CN_PHONE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|\D)(?:\+?86[-\s]?)?1[3-9]\d[-\s]?\d{4}[-\s]?\d{4}(?:$|\D)").expect("valid phone regex"));

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationError {
    pub field: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationErrors {
    pub errors: Vec<ValidationError>,
}

impl ValidationErrors {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.errors.push(ValidationError::new(field, message));
    }

    pub fn extend_with_prefix(&mut self, prefix: &str, other: ValidationErrors) {
        for err in other.errors {
            self.errors.push(ValidationError::new(
                format!("{prefix}.{}", err.field),
                err.message,
            ));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn into_result(self) -> Result<(), ValidationErrors> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

impl fmt::Display for ValidationErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (idx, err) in self.errors.iter().enumerate() {
            if idx > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{}: {}", err.field, err.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationErrors {}

pub trait Validate {
    fn validate(&self) -> Result<(), ValidationErrors>;
}

pub fn validate_patient_id(patient_id: &str) -> Result<(), ValidationError> {
    let trimmed = patient_id.trim();
    if trimmed.is_empty() {
        return Err(ValidationError::new(
            "patient_id",
            "patient_id must not be empty",
        ));
    }
    if RE_CANONICAL_PATIENT_ID.is_match(trimmed) {
        return Ok(());
    }
    if RE_EMBEDDED_ID_CARD_18.is_match(trimmed)
        || RE_EMBEDDED_ID_CARD_15.is_match(trimmed)
        || RE_ANY_18_OR_15_DIGITS.is_match(trimmed)
    {
        return Err(ValidationError::new(
            "patient_id",
            "patient_id must not contain plaintext 15/18-digit ID card numbers",
        ));
    }
    if RE_EMBEDDED_CN_PHONE.is_match(trimmed) {
        return Err(ValidationError::new(
            "patient_id",
            "patient_id must not contain plaintext 11-digit mobile phone numbers",
        ));
    }
    Ok(())
}

impl Validate for KnowledgeDocument {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.doc_id.trim().is_empty() {
            errs.push("doc_id", "doc_id must not be empty");
        }
        if self.source_title.trim().is_empty() {
            errs.push("source_title", "source_title must not be empty");
        }
        if !self.authority_level.is_finite() || !(0.1..=1.0).contains(&self.authority_level) {
            errs.push(
                "authority_level",
                "authority_level must be finite and within [0.1, 1.0]",
            );
        }
        if self.disease_name.trim().is_empty() {
            errs.push("disease_name", "disease_name must not be empty");
        }
        if self.department.trim().is_empty() {
            errs.push("department", "department must not be empty");
        }
        if self.target_population.is_empty() {
            errs.push(
                "target_population",
                "target_population must contain at least one population tag",
            );
        }
        if self.content.trim().is_empty() {
            errs.push("content", "content must not be empty");
        }
        if self.key_symptoms.is_empty() {
            errs.push("key_symptoms", "key_symptoms must not be empty");
        } else if self.key_symptoms.iter().any(|s| s.trim().is_empty()) {
            errs.push("key_symptoms", "key_symptoms must not contain blank items");
        }
        for (idx, slot) in self.suggested_follow_up_slots.iter().enumerate() {
            if slot.slot_key.trim().is_empty() {
                errs.push(
                    format!("suggested_follow_up_slots[{idx}].slot_key"),
                    "slot_key must not be empty",
                );
            }
            let q_len = slot.short_question.trim().chars().count();
            if q_len == 0 || q_len > 30 {
                errs.push(
                    format!("suggested_follow_up_slots[{idx}].short_question"),
                    "short_question must be 1..=30 characters",
                );
            }
            if !(2..=4).contains(&slot.quick_options.len()) {
                errs.push(
                    format!("suggested_follow_up_slots[{idx}].quick_options"),
                    "quick_options must contain 2..=4 options",
                );
            } else if slot.quick_options.iter().any(|o| o.trim().is_empty()) {
                errs.push(
                    format!("suggested_follow_up_slots[{idx}].quick_options"),
                    "quick_options must not contain blank items",
                );
            }
        }
        errs.into_result()
    }
}

impl Validate for VitalSignsRecord {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if let Some(s) = self.systolic_bp_mmhg
            && (!s.is_finite() || !(30.0..=320.0).contains(&s))
        {
            errs.push("systolic_bp_mmhg", "systolic_bp_mmhg must be within 30..=320 mmHg");
        }
        if let Some(d) = self.diastolic_bp_mmhg
            && (!d.is_finite() || !(15.0..=220.0).contains(&d))
        {
            errs.push("diastolic_bp_mmhg", "diastolic_bp_mmhg must be within 15..=220 mmHg");
        }
        if let Some(hr) = self.heart_rate_bpm
            && (!hr.is_finite() || !(20.0..=300.0).contains(&hr))
        {
            errs.push("heart_rate_bpm", "heart_rate_bpm must be within 20..=300 bpm");
        }
        if let Some(t) = self.temperature_c
            && (!t.is_finite() || !(30.0..=45.0).contains(&t))
        {
            errs.push("temperature_c", "temperature_c must be within 30.0..=45.0 ℃");
        }
        if let Some(spo2) = self.spo2_percent
            && (!spo2.is_finite() || !(40.0..=100.0).contains(&spo2))
        {
            errs.push("spo2_percent", "spo2_percent must be within 40..=100%");
        }
        if let Some(pain) = self.pain_score_0_10
            && pain > 10
        {
            errs.push("pain_score_0_10", "pain_score_0_10 must be within 0..=10");
        }
        errs.into_result()
    }
}

impl Validate for ClinicalMediaAsset {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.asset_id.trim().is_empty() {
            errs.push("asset_id", "asset_id must not be empty");
        }
        if self.title.trim().is_empty() {
            errs.push("title", "title must not be empty");
        }
        if self.mime_type.trim().is_empty() {
            errs.push("mime_type", "mime_type must not be empty");
        }
        errs.into_result()
    }
}

impl Validate for LabBiochemPanel {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.panel_id.trim().is_empty() {
            errs.push("panel_id", "panel_id must not be empty");
        }
        if self.panel_name.trim().is_empty() {
            errs.push("panel_name", "panel_name must not be empty");
        }
        for (idx, item) in self.items.iter().enumerate() {
            if item.item_name.trim().is_empty() {
                errs.push(
                    format!("items[{idx}].item_name"),
                    "item_name must not be empty",
                );
            }
        }
        for (idx, asset) in self.attachments.iter().enumerate() {
            if let Err(sub) = asset.validate() {
                errs.extend_with_prefix(&format!("attachments[{idx}]"), sub);
            }
        }
        errs.into_result()
    }
}

impl Validate for ImagingExamRecord {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.exam_id.trim().is_empty() {
            errs.push("exam_id", "exam_id must not be empty");
        }
        if self.exam_name.trim().is_empty() {
            errs.push("exam_name", "exam_name must not be empty");
        }
        if self.findings.trim().is_empty() && self.impression.trim().is_empty() {
            errs.push("findings", "at least one of findings or impression must be non-empty");
        }
        for (idx, img) in self.images.iter().enumerate() {
            if let Err(sub) = img.validate() {
                errs.extend_with_prefix(&format!("images[{idx}]"), sub);
            }
        }
        errs.into_result()
    }
}

impl Validate for WoundAssessmentRecord {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.wound_id.trim().is_empty() {
            errs.push("wound_id", "wound_id must not be empty");
        }
        if self.body_site.trim().is_empty() {
            errs.push("body_site", "body_site must not be empty");
        }
        if self.wound_type.trim().is_empty() {
            errs.push("wound_type", "wound_type must not be empty");
        }
        for (idx, img) in self.images.iter().enumerate() {
            if let Err(sub) = img.validate() {
                errs.extend_with_prefix(&format!("images[{idx}]"), sub);
            }
        }
        errs.into_result()
    }
}

impl Validate for TriggerSubstanceRecord {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.trigger_id.trim().is_empty() {
            errs.push("trigger_id", "trigger_id must not be empty");
        }
        if self.substance_name.trim().is_empty() {
            errs.push("substance_name", "substance_name must not be empty");
        }
        for (idx, img) in self.images.iter().enumerate() {
            if let Err(sub) = img.validate() {
                errs.extend_with_prefix(&format!("images[{idx}]"), sub);
            }
        }
        errs.into_result()
    }
}

impl Validate for ClinicalExamBundle {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        for (idx, v) in self.vital_signs.iter().enumerate() {
            if let Err(sub) = v.validate() {
                errs.extend_with_prefix(&format!("vital_signs[{idx}]"), sub);
            }
        }
        for (idx, p) in self.lab_panels.iter().enumerate() {
            if let Err(sub) = p.validate() {
                errs.extend_with_prefix(&format!("lab_panels[{idx}]"), sub);
            }
        }
        for (idx, img) in self.imaging_exams.iter().enumerate() {
            if let Err(sub) = img.validate() {
                errs.extend_with_prefix(&format!("imaging_exams[{idx}]"), sub);
            }
        }
        for (idx, w) in self.wound_assessments.iter().enumerate() {
            if let Err(sub) = w.validate() {
                errs.extend_with_prefix(&format!("wound_assessments[{idx}]"), sub);
            }
        }
        for (idx, t) in self.trigger_substances.iter().enumerate() {
            if let Err(sub) = t.validate() {
                errs.extend_with_prefix(&format!("trigger_substances[{idx}]"), sub);
            }
        }
        for (idx, m) in self.media_assets.iter().enumerate() {
            if let Err(sub) = m.validate() {
                errs.extend_with_prefix(&format!("media_assets[{idx}]"), sub);
            }
        }
        errs.into_result()
    }
}

impl Validate for L2Profile {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.age_group.trim().is_empty() {
            errs.push("age_group", "age_group must not be empty");
        }
        if let Some(ref vitals) = self.baseline_vitals
            && let Err(sub) = vitals.validate()
        {
            errs.extend_with_prefix("baseline_vitals", sub);
        }
        for (idx, t) in self.trigger_substances.iter().enumerate() {
            if let Err(sub) = t.validate() {
                errs.extend_with_prefix(&format!("trigger_substances[{idx}]"), sub);
            }
        }
        errs.into_result()
    }
}

impl Validate for L3VisitEvent {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.event_id.trim().is_empty() {
            errs.push("event_id", "event_id must not be empty");
        }
        if self.chief_complaint.trim().is_empty() {
            errs.push("chief_complaint", "chief_complaint must not be empty");
        }
        if self.recommended_department.trim().is_empty() {
            errs.push(
                "recommended_department",
                "recommended_department must not be empty",
            );
        }
        if !(1..=4).contains(&self.urgency_level) {
            errs.push("urgency_level", "urgency_level must be within 1..=4");
        }
        if self.summary.trim().is_empty() {
            errs.push("summary", "summary must not be empty");
        }
        if let Err(sub) = self.clinical_exams.validate() {
            errs.extend_with_prefix("clinical_exams", sub);
        }
        errs.into_result()
    }
}

impl Validate for PatientRecord {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if let Err(err) = validate_patient_id(&self.patient_id) {
            errs.push(err.field, err.message);
        }
        if let Err(l2_errs) = self.l2_profile.validate() {
            errs.extend_with_prefix("l2_profile", l2_errs);
        }
        let l2_fields = self
            .l2_profile
            .chronic_diseases
            .iter()
            .chain(self.l2_profile.allergies.iter())
            .chain(self.l2_profile.long_term_medications.iter())
            .chain(self.l2_profile.surgical_history.iter());
        for text in l2_fields {
            if crate::privacy::contains_high_risk_pii(text) {
                errs.push(
                    "l2_profile",
                    "l2_profile contains unredacted high-risk PII (phone/ID/email/record_no)",
                );
                break;
            }
        }
        for (idx, event) in self.l3_visit_events.iter().enumerate() {
            if let Err(l3_errs) = event.validate() {
                errs.extend_with_prefix(&format!("l3_visit_events[{idx}]"), l3_errs);
            }
            if crate::privacy::contains_high_risk_pii(&event.chief_complaint)
                || crate::privacy::contains_high_risk_pii(&event.summary)
                || event
                    .extracted_symptoms
                    .iter()
                    .any(|s| crate::privacy::contains_high_risk_pii(s))
            {
                errs.push(
                    format!("l3_visit_events[{idx}]"),
                    "l3 visit event contains unredacted high-risk PII (phone/ID/email/record_no)",
                );
            }
        }
        errs.into_result()
    }
}

impl Validate for EpidemicContext {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.context_id.trim().is_empty() {
            errs.push("context_id", "context_id must not be empty");
        }
        if self.region_code.trim().is_empty() {
            errs.push("region_code", "region_code must not be empty");
        }
        if self.region_name.trim().is_empty() {
            errs.push("region_name", "region_name must not be empty");
        }
        if self.disease_name.trim().is_empty() {
            errs.push("disease_name", "disease_name must not be empty");
        }
        if self.seasonal_tag.trim().is_empty() {
            errs.push("seasonal_tag", "seasonal_tag must not be empty");
        }
        if !self.intensity.is_finite() || !(0.0..=1.0).contains(&self.intensity) {
            errs.push("intensity", "intensity must be finite and within [0.0, 1.0]");
        }
        if self.valid_from > self.valid_to {
            errs.push("valid_from", "valid_from must be <= valid_to");
        }
        errs.into_result()
    }
}

impl Validate for SymptomSynonym {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.standard_term.trim().is_empty() {
            errs.push("standard_term", "standard_term must not be empty");
        }
        if self.colloquial_terms.is_empty() {
            errs.push(
                "colloquial_terms",
                "colloquial_terms must contain at least one term",
            );
        } else if self.colloquial_terms.iter().any(|t| t.trim().is_empty()) {
            errs.push(
                "colloquial_terms",
                "colloquial_terms must not contain blank items",
            );
        }
        errs.into_result()
    }
}

impl Validate for RedFlagRule {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        if self.rule_id.trim().is_empty() {
            errs.push("rule_id", "rule_id must not be empty");
        }
        if self.trigger_all_of.is_empty() && self.trigger_any_of.is_empty() {
            errs.push(
                "trigger_all_of",
                "at least one of trigger_all_of or trigger_any_of must be non-empty",
            );
        }
        if self.trigger_all_of.iter().any(|t| t.trim().is_empty()) {
            errs.push(
                "trigger_all_of",
                "trigger_all_of must not contain blank terms",
            );
        }
        if self.trigger_any_of.iter().any(|t| t.trim().is_empty()) {
            errs.push(
                "trigger_any_of",
                "trigger_any_of must not contain blank terms",
            );
        }
        if self.level != RedFlagLevel::None && self.alert_message.trim().is_empty() {
            errs.push(
                "alert_message",
                "alert_message must not be empty when level != None",
            );
        }
        errs.into_result()
    }
}

impl Validate for MedicalDictionaries {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        for (idx, syn) in self.symptom_synonyms.iter().enumerate() {
            if let Err(sub) = syn.validate() {
                errs.extend_with_prefix(&format!("symptom_synonyms[{idx}]"), sub);
            }
        }
        for (idx, rule) in self.red_flag_rules.iter().enumerate() {
            if let Err(sub) = rule.validate() {
                errs.extend_with_prefix(&format!("red_flag_rules[{idx}]"), sub);
            }
        }
        errs.into_result()
    }
}

impl Validate for DataCollectionPackage {
    fn validate(&self) -> Result<(), ValidationErrors> {
        let mut errs = ValidationErrors::new();
        for (idx, doc) in self.knowledge_documents.iter().enumerate() {
            if let Err(sub) = doc.validate() {
                errs.extend_with_prefix(&format!("knowledge_documents[{idx}]"), sub);
            }
        }
        for (idx, rec) in self.patient_records.iter().enumerate() {
            if let Err(sub) = rec.validate() {
                errs.extend_with_prefix(&format!("patient_records[{idx}]"), sub);
            }
        }
        for (idx, epi) in self.epidemic_contexts.iter().enumerate() {
            if let Err(sub) = epi.validate() {
                errs.extend_with_prefix(&format!("epidemic_contexts[{idx}]"), sub);
            }
        }
        if let Some(dict) = &self.medical_dictionaries
            && let Err(sub) = dict.validate()
        {
            errs.extend_with_prefix("medical_dictionaries", sub);
        }
        errs.into_result()
    }
}
