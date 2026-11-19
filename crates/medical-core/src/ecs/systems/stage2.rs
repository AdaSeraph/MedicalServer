use bevy_ecs::prelude::{Query, Res};
use chrono::Utc;

use crate::models::{ActiveEpidemicSummary, ConversationTurn};

use super::super::components::{
    ExtractedSymptoms, MemoryRecall, PatientProfile, RawUtterance, SessionMemory, TriageSlots,
};
use super::super::resources::SharedTriageResources;
use super::super::schedules::Stage2FetchedData;

pub fn memory_recall_inject_system(
    res: Res<SharedTriageResources>,
    fetched: Res<Stage2FetchedData>,
    mut query: Query<(
        &RawUtterance,
        &mut PatientProfile,
        &mut SessionMemory,
        &mut MemoryRecall,
        &mut ExtractedSymptoms,
        &mut TriageSlots,
    )>,
) {
    let Ok((raw, mut profile, mut session_mem, mut recall, mut symptoms, mut slots)) =
        query.get_single_mut()
    else {
        return;
    };

    if let Some(l2) = &fetched.l2_record {
        profile.loaded = true;
        profile.l2_profile = Some(l2.l2_profile.clone());
    } else {
        profile.loaded = false;
        profile.l2_profile = None;
    }

    recall.l3_events = fetched.l3_events.clone();
    recall.active_epidemics = fetched
        .active_epidemics
        .iter()
        .map(|e| ActiveEpidemicSummary {
            context_id: e.context_id.clone(),
            disease_name: e.disease_name.clone(),
            seasonal_tag: e.seasonal_tag.clone(),
            intensity: e.intensity,
            boost_symptoms: e.boost_symptoms.clone(),
            priority_question_hint: e.priority_question_hint.clone(),
        })
        .collect();

    let max_turns = res.0.config.max_turns.clamp(1, 4);
    slots.max_turns = max_turns;

    if let Some(prior) = &fetched.prior_session {
        session_mem.turns = prior
            .turns
            .iter()
            .map(|t| ConversationTurn {
                role: t.role.clone(),
                content: t.content.clone(),
                timestamp: t.timestamp,
            })
            .collect();
        slots.turn = prior.slots.turn_count.saturating_add(1);

        if let Some(prev_cc) = &prior.slots.chief_complaint {
            slots.chief_complaint = Some(match &slots.chief_complaint {
                Some(cur) if !cur.is_empty() && cur != prev_cc => format!("{prev_cc}；{cur}"),
                _ => prev_cc.clone(),
            });
        }
        if slots.duration.is_none() {
            slots.duration = prior.slots.duration.clone();
        }
        if slots.temperature_or_vitals.is_none() {
            slots.temperature_or_vitals = prior.slots.severity_or_temp.clone();
        }
        if slots.contact_history.is_none() {
            slots.contact_history = prior.slots.exposure_history.clone();
        }
        for sym in &prior.slots.accompanying_symptoms {
            if !slots.accompanying_symptoms.contains(sym) {
                slots.accompanying_symptoms.push(sym.clone());
            }
            if !symptoms.normalized_terms.contains(sym) {
                symptoms.normalized_terms.push(sym.clone());
            }
        }
        // Detect contact history answer from current turn (using scrubbed text)
        let current_text = if raw.scrubbed_text.trim().is_empty() {
            raw.text.as_str()
        } else {
            raw.scrubbed_text.as_str()
        };
        if slots.contact_history.is_none()
            && (current_text.contains("接触")
                || current_text.contains("家人")
                || current_text.contains("同事")
                || current_text.contains("疫苗")
                || current_text.contains("没有明确"))
        {
            slots.contact_history = Some(current_text.to_string());
        }
    } else {
        slots.turn = 1;
    }

    session_mem.turns.push(ConversationTurn {
        role: "user".to_string(),
        content: raw.scrubbed_text.clone(),
        timestamp: Utc::now(),
    });

    // Build deterministic rewritten query combining current symptoms, L2 chronic/allergies, L3, and epidemics
    let mut parts = Vec::new();
    if !symptoms.normalized_utterance.trim().is_empty() {
        parts.push(symptoms.normalized_utterance.clone());
    }
    if !symptoms.normalized_terms.is_empty() {
        parts.push(format!("症状:{}", symptoms.normalized_terms.join(" ")));
    }
    if let Some(l2) = &profile.l2_profile {
        if !l2.chronic_diseases.is_empty() {
            parts.push(format!("既往慢病:{}", l2.chronic_diseases.join(" ")));
        }
        if !l2.allergies.is_empty() {
            parts.push(format!("过敏史:{}", l2.allergies.join(" ")));
        }
    }
    if let Some(first_l3) = recall.l3_events.first() {
        parts.push(format!(
            "既往就诊:{} {}",
            first_l3.event.chief_complaint, first_l3.event.recommended_department
        ));
    }
    if !recall.active_epidemics.is_empty() {
        let epi_names: Vec<&str> = recall
            .active_epidemics
            .iter()
            .map(|e| e.disease_name.as_str())
            .collect();
        parts.push(format!("高发流行病:{}", epi_names.join(" ")));
    }

    session_mem.rewritten_query = Some(parts.join(" | "));
}
