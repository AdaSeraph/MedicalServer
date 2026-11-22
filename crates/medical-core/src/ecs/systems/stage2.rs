use bevy_ecs::prelude::{Query, Res};
use chrono::Utc;

use crate::models::{ActiveEpidemicSummary, ConversationTurn, Gender, TargetPopulation};

use super::super::components::{
    ExtractedSymptoms, MemoryRecall, PatientIdentity, PatientProfile, RawUtterance, SessionMemory,
    TriageSlots,
};
use super::super::resources::SharedTriageResources;
use super::super::schedules::Stage2FetchedData;

pub fn memory_recall_inject_system(
    res: Res<SharedTriageResources>,
    fetched: Res<Stage2FetchedData>,
    mut query: Query<(
        &mut PatientIdentity,
        &RawUtterance,
        &mut PatientProfile,
        &mut SessionMemory,
        &mut MemoryRecall,
        &mut ExtractedSymptoms,
        &mut TriageSlots,
    )>,
) {
    let Ok((mut identity, raw, mut profile, mut session_mem, mut recall, mut symptoms, mut slots)) =
        query.get_single_mut()
    else {
        return;
    };

    if let Some(l2) = &fetched.l2_record {
        profile.loaded = true;
        profile.l2_profile = Some(l2.l2_profile.clone());

        // Link L2 high-risk population attributes into PatientIdentity.target_population
        if l2.l2_profile.is_pregnant
            && !identity.target_population.contains(&TargetPopulation::Pregnant)
        {
            identity.target_population.push(TargetPopulation::Pregnant);
        }
        let age = l2.l2_profile.age_group.trim();
        let is_child = age.starts_with("0-")
            || age.starts_with("1-")
            || age.starts_with("3-")
            || age.starts_with("6-")
            || age.starts_with("7-")
            || age.contains("儿")
            || age.contains("婴")
            || age.contains("幼");
        if is_child {
            if !identity.target_population.contains(&TargetPopulation::Child) {
                identity.target_population.push(TargetPopulation::Child);
            }
            // If only default Adult was set, replace with Child so pediatric guidelines aren't masked
            if identity.target_population.len() == 2
                && identity.target_population.contains(&TargetPopulation::Adult)
            {
                identity.target_population.retain(|p| *p != TargetPopulation::Adult);
            }
        }
        let is_elderly = age.starts_with("60")
            || age.starts_with("65")
            || age.starts_with("70")
            || age.starts_with("75")
            || age.starts_with("80")
            || age.contains("老");
        if is_elderly && !identity.target_population.contains(&TargetPopulation::Elderly) {
            identity.target_population.push(TargetPopulation::Elderly);
        }
        match l2.l2_profile.gender {
            Gender::Female => {
                if !identity.target_population.contains(&TargetPopulation::Female) {
                    identity.target_population.push(TargetPopulation::Female);
                }
            }
            Gender::Male => {
                if !identity.target_population.contains(&TargetPopulation::Male) {
                    identity.target_population.push(TargetPopulation::Male);
                }
            }
            _ => {}
        }
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
        let has_family_epi_context = (current_text.contains("家人") || current_text.contains("同事"))
            && (current_text.contains("发烧")
                || current_text.contains("发热")
                || current_text.contains("咳嗽")
                || current_text.contains("感冒")
                || current_text.contains("确诊")
                || current_text.contains("阳性")
                || current_text.contains("传染")
                || current_text.contains("不适"));
        if slots.contact_history.is_none()
            && (current_text.contains("接触")
                || has_family_epi_context
                || current_text.contains("疫苗")
                || current_text.contains("没有明确")
                || current_text.contains("无明确接触"))
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
