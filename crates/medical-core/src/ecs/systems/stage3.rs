use std::collections::HashSet;

use bevy_ecs::prelude::{Query, Res};

use crate::models::Citation;

use super::super::components::{MemoryRecall, RagEvidence, RagEvidenceItem};
use super::super::resources::SharedTriageResources;
use super::super::schedules::Stage3RawCandidates;

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
            let tag_match = hit.seasonal_tags.iter().any(|t| t == &epi.seasonal_tag);
            let disease_match = hit.disease_name == epi.disease_name;
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
