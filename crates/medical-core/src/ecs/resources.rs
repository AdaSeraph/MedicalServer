use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aho_corasick::{AhoCorasick, MatchKind};
use bevy_ecs::prelude::Resource;
use jieba_rs::Jieba;

use crate::crypto::FieldCipher;
use crate::llm::{EmbeddingProvider, ModelRouter, RerankProvider};
use crate::models::{EpidemicContext, MedicalDictionaries};
use crate::storage::CoreStores;

#[derive(Debug, Clone)]
pub struct TriageEngineConfig {
    pub max_turns: u8,
    pub seasonal_alpha: f32,
    pub rag_top_k: usize,
    pub session_ttl_secs: u64,
    pub await_consolidation: bool,
    pub use_fast_query_rewrite: bool,
}

impl Default for TriageEngineConfig {
    fn default() -> Self {
        Self {
            max_turns: 4,
            seasonal_alpha: 0.35,
            rag_top_k: 5,
            session_ttl_secs: 86_400,
            await_consolidation: false,
            use_fast_query_rewrite: true,
        }
    }
}

pub struct CompiledMedicalDictionary {
    pub raw: MedicalDictionaries,
    pub jieba: Jieba,
    pub synonym_automaton: AhoCorasick,
    pub synonym_targets: Vec<String>,
    pub synonym_patterns: Vec<String>,
    pub colloquial_to_standard: HashMap<String, Vec<String>>,
    pub red_flag_automaton: AhoCorasick,
    pub red_flag_terms: Vec<String>,
}

impl std::fmt::Debug for CompiledMedicalDictionary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledMedicalDictionary")
            .field("synonyms_count", &self.raw.symptom_synonyms.len())
            .field("red_flag_rules_count", &self.raw.red_flag_rules.len())
            .field("red_flag_terms_count", &self.red_flag_terms.len())
            .finish()
    }
}

impl CompiledMedicalDictionary {
    pub fn compile(raw: MedicalDictionaries) -> Result<Self, aho_corasick::BuildError> {
        let mut jieba = Jieba::new();

        let mut synonym_patterns = Vec::new();
        let mut synonym_targets = Vec::new();
        let mut colloquial_to_standard: HashMap<String, Vec<String>> = HashMap::new();

        for syn in &raw.symptom_synonyms {
            let std_term = syn.standard_term.trim();
            if !std_term.is_empty() {
                jieba.add_word(std_term, Some(10_000), Some("nz"));
            }
            for col in &syn.colloquial_terms {
                let col_trim = col.trim();
                if !col_trim.is_empty() {
                    jieba.add_word(col_trim, Some(10_000), Some("nz"));
                    synonym_patterns.push(col_trim.to_string());
                    synonym_targets.push(std_term.to_string());
                    if !std_term.is_empty() {
                        let entry = colloquial_to_standard
                            .entry(col_trim.to_string())
                            .or_default();
                        if !entry.iter().any(|s| s == std_term) {
                            entry.push(std_term.to_string());
                        }
                    }
                }
            }
        }

        let mut seen_rf = HashSet::new();
        let mut red_flag_terms = Vec::new();
        for rule in &raw.red_flag_rules {
            for term in rule.trigger_all_of.iter().chain(rule.trigger_any_of.iter()) {
                let t = term.trim();
                if !t.is_empty() {
                    jieba.add_word(t, Some(10_000), Some("nz"));
                    if seen_rf.insert(t.to_string()) {
                        red_flag_terms.push(t.to_string());
                    }
                }
            }
        }
        // Also register standard synonym terms in red_flag_terms when they match rule triggers
        for syn in &raw.symptom_synonyms {
            let t = syn.standard_term.trim();
            if !t.is_empty() && seen_rf.contains(t) {
                for col in &syn.colloquial_terms {
                    let c = col.trim();
                    if !c.is_empty() && seen_rf.insert(c.to_string()) {
                        red_flag_terms.push(c.to_string());
                    }
                }
            }
        }

        let synonym_automaton = AhoCorasick::builder()
            .match_kind(MatchKind::LeftmostLongest)
            .build(&synonym_patterns)?;

        let red_flag_automaton = AhoCorasick::builder()
            .match_kind(MatchKind::Standard)
            .build(&red_flag_terms)?;

        Ok(Self {
            raw,
            jieba,
            synonym_automaton,
            synonym_targets,
            synonym_patterns,
            colloquial_to_standard,
            red_flag_automaton,
            red_flag_terms,
        })
    }
}

#[derive(Clone)]
pub struct InfraClients {
    pub stores: CoreStores,
    pub embedding: Arc<dyn EmbeddingProvider>,
    pub rerank: Arc<dyn RerankProvider>,
    pub cipher: Arc<FieldCipher>,
}

#[derive(Clone)]
pub struct TriageResources {
    pub dictionary: Arc<CompiledMedicalDictionary>,
    pub epidemics: Arc<Vec<EpidemicContext>>,
    pub clients: Arc<InfraClients>,
    pub model_router: Arc<ModelRouter>,
    pub config: TriageEngineConfig,
}

#[derive(Resource, Clone)]
pub struct SharedTriageResources(pub Arc<TriageResources>);
