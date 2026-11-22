use std::collections::HashSet;

use bevy_ecs::prelude::{Query, Res};

use crate::models::Citation;

use super::super::components::{MemoryRecall, RagEvidence, RagEvidenceItem};
use super::super::resources::SharedTriageResources;
use super::super::schedules::Stage3RawCandidates;

fn normalize_seasonal_tag(tag: &str) -> String {
    let lower = tag.trim().to_ascii_lowercase();
    match lower.as_str() {
        "influenza" | "flu" | "influenza_season" => "influenza_season".to_string(),
        "norovirus" | "norovirus_season" => "norovirus_season".to_string(),
        "dengue" | "dengue_season" => "dengue_season".to_string(),
        "mycoplasma" | "mycoplasma_season" => "mycoplasma_season".to_string(),
        "rsv" | "rsv_season" => "rsv_season".to_string(),
        "hfmd" | "hand_foot_mouth" => "hfmd_season".to_string(),
        _ => lower,
    }
}

fn diseases_match(doc_disease: &str, epi_disease: &str) -> bool {
    let a = doc_disease.trim().to_ascii_lowercase();
    let b = epi_disease.trim().to_ascii_lowercase();
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b
        || a.contains(&b)
        || b.contains(&a)
        || ((a.contains("流感") || a.contains("流行性感冒"))
            && (b.contains("流感") || b.contains("流行性感冒")))
}

pub fn hybrid_rag_scoring_system(
    res: Res<SharedTriageResources>,
    raw: Res<Stage3RawCandidates>,
    mut query: Query<(&MemoryRecall, &mut RagEvidence)>,
) {
    let Ok((recall, mut evidence)) = query.get_single_mut() else {
        return;
    };

    let alpha = res.0.config.seasonal_alpha.max(0.0);
    let mut items = Vec::with_capacity(raw.candidates.len());
    let mut seasonal_applied = Vec::new();
    let mut seen_seasonal_labels = HashSet::new();

    for cand in &raw.candidates {
        let hit = &cand.hit;
        let authority_multiplier = 0.7 + 0.3 * hit.authority_level.clamp(0.1, 1.0);

        let mut max_intensity = 0.0_f32;
        let mut matched_tags = Vec::new();
        for epi in &recall.active_epidemics {
            if epi.intensity <= 0.0 {
                continue;
            }
            let epi_tag_norm = normalize_seasonal_tag(&epi.seasonal_tag);
            let tag_match = hit
                .seasonal_tags
                .iter()
                .any(|t| normalize_seasonal_tag(t) == epi_tag_norm);
            let disease_match = diseases_match(&hit.disease_name, &epi.disease_name);
            if tag_match || disease_match {
                if epi.intensity > max_intensity {
                    max_intensity = epi.intensity;
                }
                if !matched_tags.contains(&epi.seasonal_tag) {
                    matched_tags.push(epi.seasonal_tag.clone());
                }
                let label = format!("{}(高发)", epi.disease_name);
                if seen_seasonal_labels.insert(label.clone()) {
                    seasonal_applied.push(label);
                }
            }
        }

        let final_score =
            cand.raw_rerank_score * authority_multiplier * (1.0 + alpha * max_intensity);

        items.push(RagEvidenceItem {
            doc_id: hit.doc_id.clone(),
            source_title: hit.source_title.clone(),
            source_type: hit.source_type,
            disease_name: hit.disease_name.clone(),
            department: hit.department.clone(),
            secondary_departments: hit.secondary_departments.clone(),
            authority_level: hit.authority_level,
            vector_score: hit.vector_score,
            bm25_score: hit.bm25_score,
            raw_rerank_score: cand.raw_rerank_score,
            seasonal_boost_applied: max_intensity,
            authority_multiplier,
            final_score,
            matched_seasonal_tags: matched_tags,
            snippet: hit.content.clone(),
            suggested_follow_up_slots: hit.suggested_follow_up_slots.clone(),
        });
    }

    items.sort_by(|a, b| {
        b.final_score
            .partial_cmp(&a.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let citations: Vec<Citation> = items
        .iter()
        .take(3)
        .map(|item| Citation {
            doc_id: item.doc_id.clone(),
            source_title: item.source_title.clone(),
            disease_name: item.disease_name.clone(),
        })
        .collect();

    evidence.items = items;
    evidence.citations = citations;
    evidence.seasonal_context_applied = seasonal_applied;
}
