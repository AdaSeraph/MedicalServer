use std::collections::HashSet;

use bevy_ecs::prelude::{Query, Res, ResMut};

use crate::models::{RedFlagLevel, TargetPopulation, TriageResultCard, TriageStatus};
use crate::privacy::{MEDICAL_DISCLAIMER, check_medical_output_violations, sanitize_medical_output};

use super::super::components::{
    ExtractedSymptoms, MemoryRecall, PatientIdentity, PatientProfile, RagEvidence, RedFlagAlert,
    TriageResponse, TriageSlots,
};
use super::super::schedules::{Stage4DecisionScratch, Stage4LlmOutcome};

pub fn slot_evaluation_system(
    mut scratch: ResMut<Stage4DecisionScratch>,
    mut query: Query<(&RedFlagAlert, &MemoryRecall, &RagEvidence, &mut TriageSlots)>,
) {
    let Ok((alert, recall, evidence, mut slots)) = query.get_single_mut() else {
        return;
    };

    slots.max_turns = slots.max_turns.clamp(1, 4);

    let top_score = evidence.items.first().map(|i| i.final_score).unwrap_or(0.0);
    let has_epidemic = recall.active_epidemics.iter().any(|e| e.intensity > 0.0);

    // Urgent red-flag must immediately converge without delaying the patient with follow-up turns
    let should_converge = alert.level == RedFlagLevel::Urgent
        || slots.turn >= slots.max_turns
        || (slots.duration.is_some()
            && (slots.temperature_or_vitals.is_some() || !slots.accompanying_symptoms.is_empty())
            && slots.turn >= 2
            && top_score >= 0.78);

    let target_slot = if should_converge {
        None
    } else if slots.duration.is_none() {
        Some("onset_duration".to_string())
    } else if has_epidemic && slots.contact_history.is_none() {
        Some("contact_history".to_string())
    } else if slots.temperature_or_vitals.is_none() {
        Some("temperature_or_vitals".to_string())
    } else {
        Some("accompanying_symptoms".to_string())
    };

    let priority_hint = if has_epidemic && slots.contact_history.is_none() {
        recall
            .active_epidemics
            .iter()
            .find(|e| e.intensity > 0.0 && !e.priority_question_hint.trim().is_empty())
            .map(|e| e.priority_question_hint.clone())
    } else {
        None
    };

    slots.next_target_slot = target_slot.clone();
    slots.priority_question_hint_injected = priority_hint.clone();

    scratch.should_converge = should_converge;
    scratch.target_slot = target_slot;
    scratch.priority_hint = priority_hint;
}

pub fn slot_question_system(
    outcome: Res<Stage4LlmOutcome>,
    scratch: Res<Stage4DecisionScratch>,
    mut query: Query<(
        &PatientIdentity,
        &PatientProfile,
        &RedFlagAlert,
        &RagEvidence,
        &TriageSlots,
        &ExtractedSymptoms,
        &mut TriageResponse,
    )>,
) {
    let Stage4LlmOutcome::Success(llm_json) = outcome.as_ref() else {
        return;
    };
    let Ok((identity, profile, alert, evidence, slots, symptoms, mut response)) =
        query.get_single_mut()
    else {
        return;
    };

    let force_complete = alert.level == RedFlagLevel::Urgent
        || slots.turn >= slots.max_turns
        || scratch.should_converge
        || llm_json.action.eq_ignore_ascii_case("completed")
        || llm_json.action.eq_ignore_ascii_case("complete");

    if force_complete {
        let top_item = evidence.items.first();
        let primary_department = llm_json
            .primary_department
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| top_item.map(|i| i.department.clone()))
            .unwrap_or_else(|| infer_department_from_symptoms(symptoms, identity, profile));

        let secondary_departments = if !llm_json.secondary_departments.is_empty() {
            llm_json.secondary_departments.clone()
        } else {
            top_item
                .map(|i| i.secondary_departments.clone())
                .unwrap_or_default()
        };

        let mut urgency_level = llm_json.urgency_level.unwrap_or(2).clamp(1, 4);
        if alert.level == RedFlagLevel::Urgent {
            urgency_level = 1;
        }

        let raw_summary = llm_json
            .summary
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                format!("结合您当前症状及知识库依据，建议尽快前往{primary_department}就诊评估。")
            });
        let sanitized_summary = sanitize_medical_output(&raw_summary, false).sanitized_text;

        response.status = TriageStatus::Completed;
        response.red_flag = alert.level;
        response.turn = slots.turn;
        response.max_turns = slots.max_turns.clamp(1, 4);
        response.question = None;
        response.quick_options.clear();
        response.seasonal_context_applied = evidence.seasonal_context_applied.clone();
        response.triage_result = Some(TriageResultCard {
            primary_department,
            secondary_departments,
            urgency_level,
            summary: sanitized_summary,
            disclaimer: MEDICAL_DISCLAIMER.to_string(),
        });
        response.citations = evidence.citations.clone();
        response.is_fallback = false;
    } else {
        let raw_q = llm_json
            .question
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| default_question_for_slot(scratch.target_slot.as_deref(), evidence))
            .unwrap_or_else(|| "症状大概持续多久了？".to_string());

        let sanitized_q = sanitize_medical_output(&raw_q, false).sanitized_text;
        let bounded_q = enforce_max_question_chars(&sanitized_q, 30);
        let options = normalize_quick_options(
            &llm_json.quick_options,
            scratch.target_slot.as_deref(),
            evidence,
        );

        response.status = TriageStatus::FollowUp;
        response.red_flag = alert.level;
        response.turn = slots.turn;
        response.max_turns = slots.max_turns.clamp(1, 4);
        response.question = Some(bounded_q);
        response.quick_options = options;
        response.seasonal_context_applied = evidence.seasonal_context_applied.clone();
        response.triage_result = None;
        response.citations = evidence.citations.clone();
        response.is_fallback = false;
    }
}

fn is_likely_non_medical_query(symptoms: &ExtractedSymptoms) -> bool {
    if !symptoms.normalized_terms.is_empty() || !symptoms.red_flag_term_hits.is_empty() {
        return false;
    }
    let lower = symptoms.normalized_utterance.to_ascii_lowercase();
    const NON_MED_MARKERS: &[&str] = &[
        "python", "javascript", "rust", "java", "c++", "sql", "代码", "算法", "快速排序",
        "写一段", "写个程序", "股票", "彩票", "天气预报", "翻译成",
    ];
    NON_MED_MARKERS.iter().any(|m| lower.contains(m))
}

pub fn fallback_rule_triage_system(
    mut query: Query<(
        &PatientIdentity,
        &PatientProfile,
        &RedFlagAlert,
        &RagEvidence,
        &TriageSlots,
        &ExtractedSymptoms,
        &mut TriageResponse,
    )>,
) {
    let Ok((identity, profile, alert, evidence, slots, symptoms, mut response)) =
        query.get_single_mut()
    else {
        return;
    };

    if is_likely_non_medical_query(symptoms) && alert.level == RedFlagLevel::None {
        response.status = TriageStatus::Completed;
        response.red_flag = RedFlagLevel::None;
        response.turn = slots.turn;
        response.max_turns = slots.max_turns.clamp(1, 4);
        response.question = None;
        response.quick_options.clear();
        response.seasonal_context_applied.clear();
        response.triage_result = Some(TriageResultCard {
            primary_department: "全科医学科".to_string(),
            secondary_departments: vec!["医院人工导诊台".to_string()],
            urgency_level: 4,
            summary: "您输入的内容似乎与医疗健康症状无关。本助手仅用于就医前科室导诊，如有身体不适，请描述您的具体症状、持续时间和伴随表现。".to_string(),
            disclaimer: MEDICAL_DISCLAIMER.to_string(),
        });
        response.citations.clear();
        response.is_fallback = true;
        return;
    }

    let top_item = evidence.items.first();
    let has_confident_evidence = top_item
        .map(|item| {
            (!symptoms.normalized_terms.is_empty() && item.final_score >= 0.45)
                || item.bm25_score.unwrap_or(0.0) > 0.0
                || item.final_score >= 0.72
        })
        .unwrap_or(false);

    let (primary_department, secondary_departments, urgency_level, low_confidence) =
        if alert.level == RedFlagLevel::Urgent {
            (
                "急诊科".to_string(),
                top_item
                    .map(|i| vec![i.department.clone()])
                    .unwrap_or_default(),
                1_u8,
                false,
            )
        } else if has_confident_evidence && let Some(item) = top_item {
            let inferred = infer_department_from_symptoms(symptoms, identity, profile);
            // Respect high-risk population specialty overrides (e.g., pregnant abdominal pain / pediatric)
            let is_pregnant = identity.target_population.contains(&TargetPopulation::Pregnant)
                || profile
                    .l2_profile
                    .as_ref()
                    .map(|l| l.is_pregnant)
                    .unwrap_or(false);
            let is_child = identity.target_population.contains(&TargetPopulation::Child);
            if (is_pregnant && inferred == "妇产科") || (is_child && inferred == "儿科") {
                (inferred, vec![item.department.clone()], 2_u8, false)
            } else {
                (
                    item.department.clone(),
                    item.secondary_departments.clone(),
                    2_u8,
                    false,
                )
            }
        } else {
            let inferred = infer_department_from_symptoms(symptoms, identity, profile);
            let is_vague = inferred == "全科医学科";
            (
                inferred,
                if is_vague {
                    vec!["医院人工导诊台".to_string()]
                } else {
                    vec!["全科医学科".to_string()]
                },
                3_u8,
                is_vague,
            )
        };

    let symptom_desc = if symptoms.normalized_terms.is_empty() {
        "当前不适症状".to_string()
    } else {
        symptoms.normalized_terms.join("、")
    };

    let raw_summary = if low_confidence {
        format!(
            "当前智能推理通道繁忙且您提供的症状信息较少，难以精准定位单一专科。建议您先前往{primary_department}或咨询医院人工导诊台，或补充具体不适部位、持续时间及伴随症状后重新问诊。"
        )
    } else {
        format!(
            "当前智能推理通道繁忙，已为您切换本地医学规则快速导诊。根据您描述的{symptom_desc}，建议优先前往{primary_department}进一步排查评估。"
        )
    };
    let sanitized_summary = sanitize_medical_output(&raw_summary, false).sanitized_text;

    response.status = TriageStatus::Completed;
    response.red_flag = alert.level;
    response.turn = slots.turn;
    response.max_turns = slots.max_turns.clamp(1, 4);
    response.question = None;
    response.quick_options.clear();
    response.seasonal_context_applied = evidence.seasonal_context_applied.clone();
    response.triage_result = Some(TriageResultCard {
        primary_department,
        secondary_departments,
        urgency_level,
        summary: sanitized_summary,
        disclaimer: MEDICAL_DISCLAIMER.to_string(),
    });
    response.citations = if low_confidence {
        Vec::new()
    } else {
        evidence.citations.clone()
    };
    response.is_fallback = true;
}

pub fn enforce_max_question_chars(input: &str, max_chars: usize) -> String {
    let trimmed = input.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = trimmed.chars().take(keep).collect();
    // Avoid dangling ASCII/punctuation before adding question mark
    while out.ends_with(['，', '、', ',', ' ', '。', '：', ':']) {
        out.pop();
    }
    out.push('？');
    out
}

fn sanitize_quick_option(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.contains(MEDICAL_DISCLAIMER) {
        return None;
    }
    if !check_medical_output_violations(trimmed).is_empty() {
        return None;
    }
    let guarded = sanitize_medical_output(trimmed, false);
    if !guarded.violations_detected.is_empty() {
        return None;
    }
    let clean = guarded.sanitized_text.trim();
    if clean.is_empty() {
        return None;
    }
    let bounded: String = if clean.chars().count() > 30 {
        clean.chars().take(30).collect()
    } else {
        clean.to_string()
    };
    if !check_medical_output_violations(&bounded).is_empty() {
        return None;
    }
    Some(bounded)
}

fn normalize_quick_options(
    raw: &[String],
    target_slot: Option<&str>,
    evidence: &RagEvidence,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for opt in raw {
        if let Some(clean) = sanitize_quick_option(opt)
            && seen.insert(clean.clone())
        {
            out.push(clean);
        }
        if out.len() == 4 {
            return out;
        }
    }

    if let Some(top) = evidence.items.first() {
        for slot in &top.suggested_follow_up_slots {
            if target_slot.map(|k| k == slot.slot_key).unwrap_or(true) {
                for opt in &slot.quick_options {
                    if let Some(clean) = sanitize_quick_option(opt)
                        && seen.insert(clean.clone())
                    {
                        out.push(clean);
                        if out.len() == 4 {
                            return out;
                        }
                    }
                }
            }
        }
    }

    let defaults: &[&str] = match target_slot {
        Some("onset_duration") => &["今天刚开始", "2~3天", "超过3天", "超过1周"],
        Some("contact_history") => &["有家人/同事发烧", "没有明确接触", "不太清楚"],
        Some("temperature_or_vitals") => &["38.5℃以上高热", "37.3~38.4℃低热", "体温正常"],
        _ => &["症状持续加重", "症状较轻微", "伴随其他不适"],
    };

    for &opt in defaults {
        if out.len() >= 2 && out.len() >= 3 {
            break;
        }
        if seen.insert(opt.to_string()) {
            out.push(opt.to_string());
        }
        if out.len() == 4 {
            break;
        }
    }
    while out.len() < 2 {
        let fallback_opt = if out.is_empty() { "是" } else { "否" };
        out.push(fallback_opt.to_string());
    }
    out.truncate(4);
    out
}

fn default_question_for_slot(target_slot: Option<&str>, evidence: &RagEvidence) -> Option<String> {
    if let Some(top) = evidence.items.first() {
        for slot in &top.suggested_follow_up_slots {
            if target_slot.map(|k| k == slot.slot_key).unwrap_or(true) {
                return Some(slot.short_question.clone());
            }
        }
    }
    Some(match target_slot {
        Some("onset_duration") => "症状大概持续多久了？".to_string(),
        Some("contact_history") => "近期接触过发热人员吗？".to_string(),
        Some("temperature_or_vitals") => "目前最高体温大概多少？".to_string(),
        _ => "还有其他明显伴随症状吗？".to_string(),
    })
}

fn infer_department_from_symptoms(
    symptoms: &ExtractedSymptoms,
    identity: &PatientIdentity,
    profile: &PatientProfile,
) -> String {
    let text = format!(
        "{} {}",
        symptoms.normalized_utterance,
        symptoms.normalized_terms.join(" ")
    );
    let is_pregnant = identity.target_population.contains(&TargetPopulation::Pregnant)
        || profile
            .l2_profile
            .as_ref()
            .map(|l| l.is_pregnant)
            .unwrap_or(false);
    let is_child = identity.target_population.contains(&TargetPopulation::Child);

    if is_pregnant
        && (text.contains("腹痛")
            || text.contains("肚子痛")
            || text.contains("出血")
            || text.contains("胎动")
            || text.contains("见红")
            || text.contains("宫缩"))
    {
        return "妇产科".to_string();
    }
    if is_child
        && (text.contains("发热")
            || text.contains("发烧")
            || text.contains("咳嗽")
            || text.contains("腹泻")
            || text.contains("皮疹")
            || text.contains("呕吐"))
    {
        return "儿科".to_string();
    }

    if text.contains("胸痛") || text.contains("心悸") || text.contains("心慌") {
        "心血管内科".to_string()
    } else if text.contains("呼吸困难")
        || text.contains("咳嗽")
        || text.contains("干咳")
        || text.contains("高热")
        || text.contains("咽痛")
        || text.contains("发热")
    {
        "呼吸内科".to_string()
    } else if text.contains("上腹痛")
        || text.contains("腹痛")
        || text.contains("腹泻")
        || text.contains("水样便")
        || text.contains("恶心")
        || text.contains("呕吐")
    {
        "消化内科".to_string()
    } else if text.contains("头晕") || text.contains("头痛") || text.contains("麻木") {
        "神经内科".to_string()
    } else {
        "全科医学科".to_string()
    }
}
