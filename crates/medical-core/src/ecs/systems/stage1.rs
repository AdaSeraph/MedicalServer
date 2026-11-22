use std::collections::HashSet;
use std::sync::LazyLock;

use bevy_ecs::prelude::{Query, Res};
use regex::Regex;

use crate::models::{RedFlagLevel, TriageResultCard, TriageStatus};
use crate::privacy::MEDICAL_DISCLAIMER;

use super::super::components::{
    ExtractedSymptoms, RagEvidence, RawUtterance, RedFlagAlert, SynonymMatch, TriageResponse,
    TriageSlots,
};
use super::super::resources::SharedTriageResources;

static RE_DURATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:持续|已经|大概|大约|约)?\s*(?:\d+(?:\.\d+)?|[一二两三四五六七八九十]+)\s*(?:天|小时|周|个月|分钟|日)")
        .expect("valid duration regex")
});
static RE_TEMPERATURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:3[5-9]|4[0-2])(?:\.\d)?\s*(?:℃|°C|度)|(?:体温\s*(?:3[5-9]|4[0-2])(?:\.\d)?)|(?:血压\s*\d{2,3}/\d{2,3})|(?:血氧\s*\d{2,3}%?)")
        .expect("valid temperature regex")
});

/// Returns `true` if `term` appears in `haystack` in at least one non-negated clause context.
pub fn has_positive_term_occurrence(haystack: &str, term: &str) -> bool {
    let t = term.trim();
    if t.is_empty() || !haystack.contains(t) {
        return false;
    }
    const NEGATION_CUES: &[&str] = &[
        "没有", "也没有", "也没", "无", "未见", "未出现", "未", "否认", "不伴", "并非", "排除",
        "不是", "不觉得", "并未", "没觉得", "不存在",
    ];
    const ADVERSATIVE_CUES: &[&str] = &["但是", "但", "而是", "却", "反而", "不过", "只是"];

    for clause in haystack.split(|c: char| {
        matches!(
            c,
            '，' | ',' | '。' | '；' | ';' | '！' | '!' | '？' | '?' | '\n' | '\r'
        )
    }) {
        for (idx, _) in clause.match_indices(t) {
            let prefix = &clause[..idx];
            // Only examine up to 16 chars before the match inside the same clause,
            // resetting after any adversative cue ("但是/但/而是/却").
            let effective_prefix = ADVERSATIVE_CUES
                .iter()
                .filter_map(|adv| prefix.rfind(adv).map(|pos| pos + adv.len()))
                .max()
                .map(|cut| &prefix[cut..])
                .unwrap_or(prefix);

            let window: String = effective_prefix
                .chars()
                .rev()
                .take(16)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();

            let is_negated = NEGATION_CUES.iter().any(|neg| window.contains(neg));
            if !is_negated {
                return true;
            }
        }
    }
    false
}

pub fn keyword_extraction_system(
    res: Res<SharedTriageResources>,
    mut query: Query<(&RawUtterance, &mut ExtractedSymptoms, &mut TriageSlots)>,
) {
    let Ok((raw, mut symptoms, mut slots)) = query.get_single_mut() else {
        return;
    };

    let dict = &res.0.dictionary;
    let text = if raw.scrubbed_text.trim().is_empty() {
        raw.text.as_str()
    } else {
        raw.scrubbed_text.as_str()
    };

    let tokens: Vec<String> = dict
        .jieba
        .cut(text, true)
        .into_iter()
        .map(|tok| tok.word.trim())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    // Colloquial -> standard replacement using LeftmostLongest automaton
    let mut normalized_utterance = String::with_capacity(text.len());
    let mut last_end = 0usize;
    let mut colloquial_matches = Vec::new();
    let mut seen_terms: HashSet<String> = HashSet::new();
    let mut normalized_terms = Vec::new();

    for mat in dict.synonym_automaton.find_iter(text) {
        if mat.start() > last_end {
            normalized_utterance.push_str(&text[last_end..mat.start()]);
        }
        let pat_idx = mat.pattern().as_usize();
        let matched_slice = &text[mat.start()..mat.end()];
        let standard = dict
            .synonym_targets
            .get(pat_idx)
            .cloned()
            .unwrap_or_else(|| matched_slice.to_string());
        normalized_utterance.push_str(&standard);
        colloquial_matches.push(SynonymMatch {
            colloquial: matched_slice.to_string(),
            standard: standard.clone(),
        });
        if has_positive_term_occurrence(text, matched_slice) && seen_terms.insert(standard.clone())
        {
            normalized_terms.push(standard);
        }
        last_end = mat.end();
    }
    if last_end < text.len() {
        normalized_utterance.push_str(&text[last_end..]);
    }

    // Add standard terms directly present in raw or normalized utterance (excluding negated terms)
    for syn in &dict.raw.symptom_synonyms {
        if (has_positive_term_occurrence(text, &syn.standard_term)
            || has_positive_term_occurrence(&normalized_utterance, &syn.standard_term))
            && seen_terms.insert(syn.standard_term.clone())
        {
            normalized_terms.push(syn.standard_term.clone());
        }
    }

    // Scan BOTH scrubbed and normalized utterances with Standard overlapping automaton,
    // filtering out negated occurrences (e.g., "没有胸痛，也没有大汗和呼吸困难").
    let mut rf_hits = Vec::new();
    let mut rf_seen = HashSet::new();
    for haystack in [text, normalized_utterance.as_str()] {
        for mat in dict.red_flag_automaton.find_overlapping_iter(haystack) {
            if let Some(term) = dict.red_flag_terms.get(mat.pattern().as_usize())
                && has_positive_term_occurrence(haystack, term)
                && rf_seen.insert(term.clone())
            {
                rf_hits.push(term.clone());
            }
        }
    }
    for term in &rf_hits {
        if seen_terms.insert(term.clone()) {
            normalized_terms.push(term.clone());
        }
        if let Some(standards) = dict.colloquial_to_standard.get(term.as_str()) {
            for std_term in standards {
                if seen_terms.insert(std_term.clone()) {
                    normalized_terms.push(std_term.clone());
                }
            }
        }
    }

    let inferred_duration = RE_DURATION.find(text).map(|m| m.as_str().trim().to_string());
    let inferred_temperature = RE_TEMPERATURE
        .find(text)
        .map(|m| m.as_str().trim().to_string());

    if slots.chief_complaint.is_none() {
        slots.chief_complaint = Some(normalized_utterance.clone());
    }
    if slots.duration.is_none() {
        slots.duration = inferred_duration.clone();
    }
    if slots.temperature_or_vitals.is_none() {
        slots.temperature_or_vitals = inferred_temperature.clone();
    }
    for term in &normalized_terms {
        if !slots.accompanying_symptoms.contains(term) {
            slots.accompanying_symptoms.push(term.clone());
        }
    }

    symptoms.tokens = tokens;
    symptoms.colloquial_matches = colloquial_matches;
    symptoms.normalized_terms = normalized_terms;
    symptoms.normalized_utterance = normalized_utterance;
    symptoms.red_flag_term_hits = rf_hits;
    symptoms.inferred_duration = inferred_duration;
    symptoms.inferred_temperature = inferred_temperature;
}

pub fn red_flag_guard_system(
    res: Res<SharedTriageResources>,
    mut query: Query<(
        &RawUtterance,
        &ExtractedSymptoms,
        &TriageSlots,
        &mut RedFlagAlert,
    )>,
) {
    let Ok((raw, symptoms, slots, mut alert)) = query.get_single_mut() else {
        return;
    };

    let dict = &res.0.dictionary;
    let mut term_set: HashSet<&str> = HashSet::new();
    for t in &symptoms.normalized_terms {
        term_set.insert(t.as_str());
    }
    for t in &symptoms.red_flag_term_hits {
        term_set.insert(t.as_str());
        if let Some(standards) = dict.colloquial_to_standard.get(t.as_str()) {
            for s in standards {
                term_set.insert(s.as_str());
            }
        }
    }
    for t in &slots.accompanying_symptoms {
        term_set.insert(t.as_str());
    }

    let raw_text = if raw.scrubbed_text.trim().is_empty() {
        raw.text.as_str()
    } else {
        raw.scrubbed_text.as_str()
    };

    let matches_term = |term: &str| -> bool {
        let t = term.trim();
        !t.is_empty()
            && (term_set.contains(t)
                || has_positive_term_occurrence(raw_text, t)
                || has_positive_term_occurrence(&symptoms.normalized_utterance, t)
                || slots
                    .chief_complaint
                    .as_deref()
                    .map(|c| has_positive_term_occurrence(c, t))
                    .unwrap_or(false))
    };

    let mut best_level = RedFlagLevel::None;
    let mut best_rule_id = None;
    let mut best_message = None;
    let mut best_terms = Vec::new();

    for rule in &res.0.dictionary.raw.red_flag_rules {
        if rule.level == RedFlagLevel::None {
            continue;
        }
        let all_ok = rule.trigger_all_of.iter().all(|t| matches_term(t));
        let any_ok =
            rule.trigger_any_of.is_empty() || rule.trigger_any_of.iter().any(|t| matches_term(t));

        if all_ok && any_ok && rule.level > best_level {
            best_level = rule.level;
            best_rule_id = Some(rule.rule_id.clone());
            best_message = Some(rule.alert_message.clone());
            best_terms = rule
                .trigger_all_of
                .iter()
                .chain(rule.trigger_any_of.iter())
                .filter(|t| matches_term(t))
                .cloned()
                .collect();
        }
    }

    alert.level = best_level;
    alert.matched_rule_id = best_rule_id;
    alert.matched_terms = best_terms;
    alert.alert_message = best_message;
}

pub fn emergency_short_circuit_response_system(
    mut query: Query<(
        &RedFlagAlert,
        &TriageSlots,
        &RagEvidence,
        &mut TriageResponse,
    )>,
) {
    let Ok((alert, slots, evidence, mut response)) = query.get_single_mut() else {
        return;
    };
    if !alert.is_emergency_120() {
        return;
    }

    let summary = alert.alert_message.clone().unwrap_or_else(|| {
        "检测到急危重症预警信号，请立即停止等待并拨打 120 或前往最近医院急诊科就诊！".to_string()
    });

    response.status = TriageStatus::Emergency;
    response.red_flag = RedFlagLevel::Emergency120;
    response.turn = slots.turn.max(1);
    response.max_turns = slots.max_turns.clamp(1, 4);
    response.question = None;
    response.quick_options.clear();
    response.seasonal_context_applied = evidence.seasonal_context_applied.clone();
    response.triage_result = Some(TriageResultCard {
        primary_department: "急诊科".to_string(),
        secondary_departments: vec!["急救中心(120)".to_string()],
        urgency_level: 1,
        summary,
        disclaimer: MEDICAL_DISCLAIMER.to_string(),
    });
    response.citations = evidence.citations.clone();
    response.is_fallback = false;
}
