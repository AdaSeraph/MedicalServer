use std::sync::Arc;

use bevy_ecs::prelude::Query;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::llm::{LlmMessage, LlmRequest, ModelTier};
use crate::models::{L2Profile, L3VisitEvent, TriageStatus};
use crate::privacy::scrub_pii;

use super::super::components::{
    ConsolidationPlan, ExtractedSymptoms, PatientIdentity, PatientProfile, RawUtterance,
    TriageResponse, TriageSlots,
};
use super::super::resources::TriageResources;

pub fn memory_consolidation_plan_system(
    mut query: Query<(
        &PatientIdentity,
        &TriageResponse,
        &mut ConsolidationPlan,
    )>,
) {
    let Ok((identity, response, mut plan)) = query.get_single_mut() else {
        return;
    };

    if identity.consent_long_term_memory && response.status == TriageStatus::Completed {
        plan.should_consolidate = true;
        plan.reason = "patient consented and triage completed".to_string();
    } else {
        plan.should_consolidate = false;
        plan.reason = if !identity.consent_long_term_memory {
            "patient did not consent to long-term memory".to_string()
        } else {
            "triage not yet completed".to_string()
        };
    }
}

#[derive(Debug, Deserialize)]
struct ConsolidationExtractJson {
    #[serde(default)]
    chronic_diseases_add: Vec<String>,
    #[serde(default)]
    allergies_add: Vec<String>,
    #[serde(default)]
    summary: Option<String>,
}

pub struct ConsolidationInput {
    pub patient_id: String,
    pub consent: bool,
    pub existing_l2: Option<L2Profile>,
    pub chief_complaint: String,
    pub extracted_symptoms: Vec<String>,
    pub recommended_department: String,
    pub urgency_level: u8,
    pub triage_summary: String,
}

pub async fn execute_memory_consolidation(
    res: Arc<TriageResources>,
    input: ConsolidationInput,
) -> bool {
    if !input.consent {
        return false;
    }

    let mut l2 = input.existing_l2.unwrap_or_else(|| L2Profile {
        age_group: "unknown".to_string(),
        ..Default::default()
    });

    let mut summary = scrub_pii(&input.triage_summary).scrubbed_text;

    // Optional Fast-Tier LLM incremental L2/L3 summary extraction (with deterministic fallback)
    let prompt = format!(
        "请从以下分诊对话提取新增慢病、过敏史及一句话就诊摘要（严格JSON: {{\"chronic_diseases_add\":[],\"allergies_add\":[],\"summary\":\"\"}}）：\n主诉：{}\n症状：{:?}\n分诊建议：{}",
        input.chief_complaint, input.extracted_symptoms, summary
    );
    let req = LlmRequest {
        messages: vec![
            LlmMessage::system("你是医疗病历结构化助手，仅输出JSON，不得包含真实姓名、身份证或手机号。"),
            LlmMessage::user(prompt),
        ],
        tier: ModelTier::Fast,
        temperature: Some(0.1),
        max_tokens: Some(256),
        response_format_json: true,
        json_schema: None,
        timeout_ms: Some(3000),
    };

    if let Ok(resp) = res.model_router.complete_json(&req).await
        && let Ok(parsed) = serde_json::from_str::<ConsolidationExtractJson>(&resp.content)
    {
        for item in parsed.chronic_diseases_add {
            let t = item.trim();
            if !t.is_empty() && !l2.chronic_diseases.iter().any(|x| x == t) {
                l2.chronic_diseases.push(t.to_string());
            }
        }
        for item in parsed.allergies_add {
            let t = item.trim();
            if !t.is_empty() && !l2.allergies.iter().any(|x| x == t) {
                l2.allergies.push(t.to_string());
            }
        }
        if let Some(s) = parsed.summary
            && !s.trim().is_empty()
        {
            summary = scrub_pii(&s).scrubbed_text;
        }
    }

    let chief = scrub_pii(&input.chief_complaint).scrubbed_text;
    let event = L3VisitEvent {
        event_id: format!("evt_{}", Uuid::new_v4().simple()),
        occurred_at: Utc::now(),
        chief_complaint: chief.clone(),
        extracted_symptoms: input.extracted_symptoms,
        recommended_department: input.recommended_department,
        urgency_level: input.urgency_level.clamp(1, 4),
        summary: summary.clone(),
        clinical_exams: Default::default(),
    };

    let l2_ok = res
        .clients
        .stores
        .patient_memory
        .upsert_l2_profile(&input.patient_id, true, &l2)
        .await
        .is_ok();
    let l3_ok = res
        .clients
        .stores
        .patient_memory
        .insert_l3_event(&input.patient_id, &event)
        .await
        .is_ok();

    if l3_ok
        && let Ok(vec) = res
            .clients
            .embedding
            .embed_one(&format!("{chief} {summary}"))
            .await
    {
        let _ = res
            .clients
            .stores
            .patient_l3_vectors
            .upsert_event_vector(&input.patient_id, &event, vec)
            .await;
    }

    l2_ok && l3_ok
}

pub fn build_consolidation_input(
    identity: &PatientIdentity,
    raw: &RawUtterance,
    profile: &PatientProfile,
    symptoms: &ExtractedSymptoms,
    slots: &TriageSlots,
    response: &TriageResponse,
) -> Option<ConsolidationInput> {
    let card = response.triage_result.as_ref()?;
    Some(ConsolidationInput {
        patient_id: identity.patient_id.clone(),
        consent: identity.consent_long_term_memory,
        existing_l2: profile.l2_profile.clone(),
        chief_complaint: slots
            .chief_complaint
            .clone()
            .unwrap_or_else(|| raw.scrubbed_text.clone()),
        extracted_symptoms: symptoms.normalized_terms.clone(),
        recommended_department: card.primary_department.clone(),
        urgency_level: card.urgency_level,
        triage_summary: card.summary.clone(),
    })
}
