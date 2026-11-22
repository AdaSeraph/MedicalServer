use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use medical_core::llm::{EmbeddingProvider, RerankCandidate, RerankProvider};
use medical_core::models::{EpidemicContext, KnowledgeDocument};
use medical_core::storage::{
    CheckpointStatus, CheckpointStore, KnowledgeSearchEngine, KnowledgeSearchHit,
    KnowledgeSearchQuery, KnowledgeVectorFilter, KnowledgeVectorStore, StorageError, StorageResult,
    VectorizedKnowledgeChunk,
};
use tantivy::collector::{DocSetCollector, TopDocs};
use tantivy::query::{AllQuery, QueryParser};
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};
use tantivy_jieba::JiebaTokenizer;

use crate::checkpoint::{CheckpointManager, JobProgress};
use crate::error::{PipelineError, PipelineResult};
use crate::json_stream::{count_jsonl_lines, stream_knowledge_jsonl};

pub const DEFAULT_SEASONAL_ALPHA: f32 = 0.35;
const JIEBA_TOKENIZER_NAME: &str = "jieba_zh";

#[derive(Clone, Copy)]
struct TantivyFields {
    doc_id: Field,
    disease_name: Field,
    department: Field,
    target_population: Field,
    tenant_namespace: Field,
    seasonal_tags: Field,
    key_symptoms: Field,
    content: Field,
    payload_json: Field,
}

/// Full-text BM25 index backed by `tantivy` + `tantivy-jieba` Chinese medical tokenization.
pub struct TantivyKnowledgeIndex {
    index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    fields: TantivyFields,
}

impl TantivyKnowledgeIndex {
    pub fn in_memory() -> PipelineResult<Self> {
        let (schema, fields) = build_schema();
        let index = Index::create_in_ram(schema);
        Self::finish_init(index, fields)
    }

    pub fn open_or_create(dir: impl AsRef<Path>) -> PipelineResult<Self> {
        std::fs::create_dir_all(dir.as_ref())?;
        let (schema, fields) = build_schema();
        let index = Index::open_or_create(
            tantivy::directory::MmapDirectory::open(dir.as_ref())
                .map_err(|e| PipelineError::Index(e.to_string()))?,
            schema,
        )
        .map_err(|e| PipelineError::Index(e.to_string()))?;
        Self::finish_init(index, fields)
    }

    fn finish_init(index: Index, fields: TantivyFields) -> PipelineResult<Self> {
        index
            .tokenizers()
            .register(JIEBA_TOKENIZER_NAME, JiebaTokenizer::new());
        let writer = index
            .writer(25_000_000)
            .map_err(|e| PipelineError::Index(e.to_string()))?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(|e| PipelineError::Index(e.to_string()))?;
        Ok(Self {
            index,
            reader,
            writer: Mutex::new(writer),
            fields,
        })
    }

    /// Idempotently upserts `KnowledgeDocument`s by `doc_id` (deletes prior term, then inserts).
    pub fn upsert_documents(
        &self,
        docs: &[KnowledgeDocument],
        tenant_namespace: &str,
    ) -> PipelineResult<u64> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| PipelineError::Index("tantivy writer lock poisoned".to_string()))?;

        let mut count = 0u64;
        for d in docs {
            let scoped_key = format!("{tenant_namespace}:{}", d.doc_id);
            let term = Term::from_field_text(self.fields.doc_id, &scoped_key);
            writer.delete_term(term);

            let payload_json = serde_json::to_string(d)
                .map_err(|e| PipelineError::Index(format!("serialize doc payload: {e}")))?;

            let disease_with_aliases = if d.aliases.is_empty() {
                d.disease_name.clone()
            } else {
                format!("{} {}", d.disease_name, d.aliases.join(" "))
            };
            let content_with_diff = if d.differential_points.is_empty() {
                d.content.clone()
            } else {
                format!("{} {}", d.content, d.differential_points.join(" "))
            };

            let mut tdoc = doc!(
                self.fields.doc_id => scoped_key.as_str(),
                self.fields.disease_name => disease_with_aliases.as_str(),
                self.fields.department => d.department.as_str(),
                self.fields.tenant_namespace => tenant_namespace,
                self.fields.key_symptoms => d.key_symptoms.join(" "),
                self.fields.content => content_with_diff.as_str(),
                self.fields.payload_json => payload_json.as_str()
            );
            for pop in &d.target_population {
                tdoc.add_text(self.fields.target_population, pop.as_str());
            }
            for tag in &d.seasonal_tags {
                tdoc.add_text(self.fields.seasonal_tags, tag.as_str());
            }
            writer
                .add_document(tdoc)
                .map_err(|e| PipelineError::Index(e.to_string()))?;
            count += 1;
        }

        writer
            .commit()
            .map_err(|e| PipelineError::Index(e.to_string()))?;
        self.reader
            .reload()
            .map_err(|e| PipelineError::Index(e.to_string()))?;
        Ok(count)
    }

    pub fn delete_by_doc_ids(&self, doc_ids: &[String]) -> PipelineResult<u64> {
        self.delete_by_doc_ids_in_namespace(doc_ids, "default")
    }

    pub fn delete_by_doc_ids_in_namespace(
        &self,
        doc_ids: &[String],
        tenant_namespace: &str,
    ) -> PipelineResult<u64> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| PipelineError::Index("tantivy writer lock poisoned".to_string()))?;
        for id in doc_ids {
            let scoped_key = format!("{tenant_namespace}:{id}");
            writer.delete_term(Term::from_field_text(self.fields.doc_id, &scoped_key));
            writer.delete_term(Term::from_field_text(self.fields.doc_id, id));
        }
        writer
            .commit()
            .map_err(|e| PipelineError::Index(e.to_string()))?;
        self.reader
            .reload()
            .map_err(|e| PipelineError::Index(e.to_string()))?;
        Ok(doc_ids.len() as u64)
    }

    /// Returns total committed documents in the Tantivy index.
    pub fn num_docs(&self) -> u64 {
        self.reader.searcher().num_docs()
    }

    /// Lists all committed `KnowledgeDocument`s in the index, optionally filtered by tenant namespace.
    pub fn list_all_documents(
        &self,
        tenant_namespace: Option<&str>,
    ) -> PipelineResult<Vec<KnowledgeDocument>> {
        let searcher = self.reader.searcher();
        let doc_addrs = searcher
            .search(&AllQuery, &DocSetCollector)
            .map_err(|e| PipelineError::Index(e.to_string()))?;

        let mut seen = HashSet::new();
        let mut docs = Vec::with_capacity(doc_addrs.len());
        for doc_addr in doc_addrs {
            let retrieved: TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| PipelineError::Index(e.to_string()))?;
            let doc_ns = retrieved
                .get_first(self.fields.tenant_namespace)
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            if let Some(ns) = tenant_namespace.map(str::trim).filter(|s| !s.is_empty()) {
                let is_public = doc_ns == "default" || doc_ns == "public";
                if !is_public && doc_ns != ns {
                    continue;
                }
            }
            let Some(payload_val) = retrieved
                .get_first(self.fields.payload_json)
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            if let Ok(doc) = serde_json::from_str::<KnowledgeDocument>(payload_val)
                && seen.insert(doc.doc_id.clone())
            {
                docs.push(doc);
            }
        }
        docs.sort_by(|a, b| {
            a.department
                .cmp(&b.department)
                .then_with(|| a.disease_name.cmp(&b.disease_name))
                .then_with(|| a.doc_id.cmp(&b.doc_id))
        });
        Ok(docs)
    }

    /// Executes BM25 full-text search with hard metadata filters.
    pub fn search_bm25(&self, query: &KnowledgeSearchQuery) -> PipelineResult<Vec<KnowledgeSearchHit>> {
        let searcher = self.reader.searcher();
        let mut combined_text = query.query_text.clone();
        if !query.keywords.is_empty() {
            combined_text.push(' ');
            combined_text.push_str(&query.keywords.join(" "));
        }
        let cleaned = combined_text.trim();
        if cleaned.is_empty() {
            return Ok(Vec::new());
        }

        let mut parser = QueryParser::for_index(
            &self.index,
            vec![
                self.fields.disease_name,
                self.fields.key_symptoms,
                self.fields.content,
            ],
        );
        parser.set_field_boost(self.fields.disease_name, 2.5);
        parser.set_field_boost(self.fields.key_symptoms, 2.0);
        parser.set_field_boost(self.fields.content, 1.0);

        let (parsed_query, _errors) = parser.parse_query_lenient(cleaned);
        // Fetch sufficient candidates so hard metadata filtering for niche populations still yields `top_k`
        let fetch_limit = (query.top_k.max(1) * 20).max(100);
        let top_docs = searcher
            .search(
                &parsed_query,
                &TopDocs::with_limit(fetch_limit).order_by_score(),
            )
            .map_err(|e| PipelineError::Index(e.to_string()))?;

        let mut hits = Vec::new();
        for (score, doc_addr) in top_docs {
            let retrieved: TantivyDocument = searcher
                .doc(doc_addr)
                .map_err(|e| PipelineError::Index(e.to_string()))?;
            let Some(payload_val) = retrieved
                .get_first(self.fields.payload_json)
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            let tenant_ns = retrieved
                .get_first(self.fields.tenant_namespace)
                .and_then(|v| v.as_str())
                .unwrap_or("default")
                .to_string();

            let Ok(doc) = serde_json::from_str::<KnowledgeDocument>(payload_val) else {
                continue;
            };

            if !matches_hard_filters(&doc, &tenant_ns, query) {
                continue;
            }

            hits.push(KnowledgeSearchHit {
                doc_id: doc.doc_id,
                source_title: doc.source_title,
                source_type: doc.source_type,
                authority_level: doc.authority_level,
                disease_name: doc.disease_name,
                department: doc.department,
                secondary_departments: doc.secondary_departments,
                target_population: doc.target_population,
                section_type: doc.section_type,
                content: doc.content,
                key_symptoms: doc.key_symptoms,
                red_flag_signs: doc.red_flag_signs,
                seasonal_tags: doc.seasonal_tags,
                suggested_follow_up_slots: doc.suggested_follow_up_slots,
                vector_score: None,
                bm25_score: Some(score),
                rrf_score: score,
                tenant_namespace: Some(tenant_ns),
            });

            if hits.len() >= query.top_k.max(1) {
                break;
            }
        }

        Ok(hits)
    }
}

fn build_schema() -> (Schema, TantivyFields) {
    let mut builder = Schema::builder();
    let zh_text_options = TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(JIEBA_TOKENIZER_NAME)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions),
    );

    let doc_id = builder.add_text_field("doc_id", STRING | STORED);
    let disease_name = builder.add_text_field("disease_name", zh_text_options.clone());
    let department = builder.add_text_field("department", STRING | STORED);
    let target_population = builder.add_text_field("target_population", STRING);
    let tenant_namespace = builder.add_text_field("tenant_namespace", STRING | STORED);
    let seasonal_tags = builder.add_text_field("seasonal_tags", STRING);
    let key_symptoms = builder.add_text_field("key_symptoms", zh_text_options.clone());
    let content = builder.add_text_field("content", zh_text_options);
    let payload_json = builder.add_text_field("payload_json", STORED);

    let fields = TantivyFields {
        doc_id,
        disease_name,
        department,
        target_population,
        tenant_namespace,
        seasonal_tags,
        key_symptoms,
        content,
        payload_json,
    };
    (builder.build(), fields)
}

fn matches_hard_filters(
    doc: &KnowledgeDocument,
    doc_tenant_ns: &str,
    query: &KnowledgeSearchQuery,
) -> bool {
    if let Some(dept) = &query.department
        && doc.department != *dept
        && !doc.secondary_departments.contains(dept)
    {
        return false;
    }
    if let Some(pops) = &query.target_population
        && !pops.is_empty()
        && !doc.target_population.iter().any(|p| pops.contains(p))
    {
        return false;
    }
    if let Some(ns) = &query.tenant_namespace {
        let is_public = doc_tenant_ns == "default" || doc_tenant_ns == "public";
        if !is_public && doc_tenant_ns != ns {
            return false;
        }
    }
    true
}

/// Dual-write indexer that embeds documents and atomically upserts them into
/// both `KnowledgeVectorStore` (Qdrant / InMemory) and `TantivyKnowledgeIndex`.
pub struct DualWriteIndexer {
    embedder: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn KnowledgeVectorStore>,
    tantivy: Arc<TantivyKnowledgeIndex>,
    checkpoints: Option<CheckpointManager>,
    batch_size: usize,
}

impl DualWriteIndexer {
    pub fn new(
        embedder: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn KnowledgeVectorStore>,
        tantivy: Arc<TantivyKnowledgeIndex>,
    ) -> Self {
        Self {
            embedder,
            vector_store,
            tantivy,
            checkpoints: None,
            batch_size: 32,
        }
    }

    pub fn with_checkpoint_store(mut self, store: Arc<dyn CheckpointStore>) -> Self {
        self.checkpoints = Some(CheckpointManager::new(store));
        self
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Idempotently upserts `KnowledgeDocument`s into both VectorStore and Tantivy by `doc_id`.
    pub async fn upsert_documents(
        &self,
        docs: &[KnowledgeDocument],
        tenant_namespace: &str,
    ) -> PipelineResult<u64> {
        if docs.is_empty() {
            return Ok(0);
        }

        // Deduplicate within batch keeping last occurrence per doc_id
        let mut dedup_map: HashMap<&str, &KnowledgeDocument> = HashMap::new();
        let mut ordered_ids: Vec<&str> = Vec::new();
        for d in docs {
            if !dedup_map.contains_key(d.doc_id.as_str()) {
                ordered_ids.push(d.doc_id.as_str());
            }
            dedup_map.insert(d.doc_id.as_str(), d);
        }
        let unique_docs: Vec<KnowledgeDocument> = ordered_ids
            .into_iter()
            .filter_map(|id| dedup_map.get(id).map(|d| (*d).clone()))
            .collect();

        let mut total_written = 0u64;
        for chunk in unique_docs.chunks(self.batch_size) {
            let texts: Vec<String> = chunk
                .iter()
                .map(|d| {
                    format!(
                        "{} {} {} {}",
                        d.disease_name,
                        d.department,
                        d.key_symptoms.join(" "),
                        d.content
                    )
                })
                .collect();

            let vectors = self.embedder.embed_batch(&texts).await?;
            if vectors.len() != chunk.len() {
                return Err(PipelineError::Index(format!(
                    "embedding batch size mismatch: expected {}, got {}",
                    chunk.len(),
                    vectors.len()
                )));
            }

            let vectorized: Vec<VectorizedKnowledgeChunk> = chunk
                .iter()
                .cloned()
                .zip(vectors.into_iter())
                .map(|(document, vector)| VectorizedKnowledgeChunk {
                    document,
                    vector,
                    tenant_namespace: tenant_namespace.to_string(),
                })
                .collect();

            let tantivy = Arc::clone(&self.tantivy);
            let chunk_owned = chunk.to_vec();
            let ns_owned = tenant_namespace.to_string();
            let (vec_res, bm25_res) = tokio::join!(
                self.vector_store.upsert_documents(&vectorized),
                tokio::task::spawn_blocking(move || tantivy.upsert_documents(&chunk_owned, &ns_owned))
            );
            vec_res?;
            bm25_res
                .map_err(|e| PipelineError::Index(format!("tantivy blocking join error: {e}")))??;
            total_written += chunk.len() as u64;
        }

        Ok(total_written)
    }

    /// Streams a JSONL knowledge file with batch dual-write and resumable checkpoints (断点续传).
    ///
    /// Pass `max_batches: Some(n)` to pause after `n` batches (useful for resumable/pause testing).
    pub async fn ingest_knowledge_jsonl_resumable(
        &self,
        job_id: &str,
        path: impl AsRef<Path>,
        tenant_namespace: &str,
        max_batches: Option<usize>,
    ) -> PipelineResult<JobProgress> {
        let started_at = Instant::now();
        let path_ref = path.as_ref();
        let source_path = path_ref.to_string_lossy().to_string();
        let total_items = count_jsonl_lines(path_ref)?;

        let mut cp = if let Some(mgr) = &self.checkpoints {
            mgr.load_or_init(job_id, &source_path, "knowledge_jsonl", total_items)
                .await?
        } else {
            medical_core::storage::IngestionCheckpoint {
                job_id: job_id.to_string(),
                source_path: source_path.clone(),
                job_type: "knowledge_jsonl".to_string(),
                status: CheckpointStatus::Pending,
                total_items,
                processed_items: 0,
                failed_items: 0,
                byte_offset: 0,
                last_doc_id: None,
                error_samples: Vec::new(),
                updated_at: chrono::Utc::now(),
            }
        };

        if cp.status == CheckpointStatus::Completed {
            return Ok(CheckpointManager::build_progress(&cp, started_at));
        }

        let mut batches_done = 0usize;
        loop {
            if let Some(limit) = max_batches
                && batches_done >= limit
            {
                if let Some(mgr) = &self.checkpoints {
                    mgr.save_progress(&mut cp, CheckpointStatus::Paused).await?;
                } else {
                    cp.status = CheckpointStatus::Paused;
                }
                return Ok(CheckpointManager::build_progress(&cp, started_at));
            }

            let report = stream_knowledge_jsonl(path_ref, cp.byte_offset, Some(self.batch_size))?;
            if report.lines_read == 0 {
                if let Some(mgr) = &self.checkpoints {
                    mgr.save_progress(&mut cp, CheckpointStatus::Completed)
                        .await?;
                } else {
                    cp.status = CheckpointStatus::Completed;
                }
                return Ok(CheckpointManager::build_progress(&cp, started_at));
            }

            for err in &report.errors {
                cp.failed_items += 1;
                cp.error_samples.push(format!(
                    "line {} (offset {}): {}",
                    err.line_number, err.byte_offset, err.message
                ));
            }

            if !report.records.is_empty() {
                let written = self
                    .upsert_documents(&report.records, tenant_namespace)
                    .await?;
                cp.processed_items += written;
                cp.last_doc_id = report.records.last().map(|d| d.doc_id.clone());
            }

            cp.byte_offset = report.end_byte_offset;
            batches_done += 1;

            if let Some(mgr) = &self.checkpoints {
                mgr.save_progress(&mut cp, CheckpointStatus::Running)
                    .await?;
            }
        }
    }
}

/// Hybrid search engine combining Vector recall (Qdrant/InMemory) + Tantivy BM25 via RRF,
/// with optional Reranker and seasonal epidemic dynamic boosting (`alpha = 0.35`).
pub struct HybridKnowledgeSearchEngine {
    embedder: Arc<dyn EmbeddingProvider>,
    vector_store: Arc<dyn KnowledgeVectorStore>,
    tantivy: Arc<TantivyKnowledgeIndex>,
    reranker: Option<Arc<dyn RerankProvider>>,
    rrf_k: f32,
    seasonal_alpha: f32,
}

impl HybridKnowledgeSearchEngine {
    pub fn new(
        embedder: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn KnowledgeVectorStore>,
        tantivy: Arc<TantivyKnowledgeIndex>,
    ) -> Self {
        Self {
            embedder,
            vector_store,
            tantivy,
            reranker: None,
            rrf_k: 60.0,
            seasonal_alpha: DEFAULT_SEASONAL_ALPHA,
        }
    }

    pub fn with_reranker(mut self, reranker: Arc<dyn RerankProvider>) -> Self {
        self.reranker = Some(reranker);
        self
    }

    pub fn with_seasonal_alpha(mut self, alpha: f32) -> Self {
        self.seasonal_alpha = alpha;
        self
    }

    /// Runs hybrid RRF search and applies seasonal epidemic boosting:
    /// `final_score = score * (1.0 + alpha * epidemic_intensity)`.
    pub async fn search_with_seasonal_boost(
        &self,
        query: &KnowledgeSearchQuery,
        active_epidemics: &[EpidemicContext],
        alpha_override: Option<f32>,
    ) -> PipelineResult<(Vec<KnowledgeSearchHit>, Vec<String>)> {
        let mut hits = self.search(query).await?;
        let alpha = alpha_override.unwrap_or(self.seasonal_alpha);

        let mut applied_contexts: HashSet<String> = HashSet::new();
        if !active_epidemics.is_empty() && alpha > 0.0 {
            for hit in &mut hits {
                let mut max_intensity = 0.0f32;
                for epi in active_epidemics {
                    if epi.intensity <= 0.0 {
                        continue;
                    }
                    let tag_match = hit.seasonal_tags.iter().any(|t| {
                        t.eq_ignore_ascii_case(&epi.seasonal_tag)
                            || t.contains(&epi.seasonal_tag)
                    }) || hit.disease_name.contains(&epi.disease_name);
                    if tag_match {
                        applied_contexts.insert(epi.disease_name.clone());
                        if epi.intensity > max_intensity {
                            max_intensity = epi.intensity;
                        }
                    }
                }
                if max_intensity > 0.0 {
                    hit.rrf_score *= 1.0 + alpha * max_intensity;
                }
            }
            hits.sort_by(|a, b| {
                b.rrf_score
                    .partial_cmp(&a.rrf_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        let mut applied_vec: Vec<String> = applied_contexts.into_iter().collect();
        applied_vec.sort();
        Ok((hits, applied_vec))
    }
}

#[async_trait]
impl KnowledgeSearchEngine for HybridKnowledgeSearchEngine {
    async fn search(&self, query: &KnowledgeSearchQuery) -> StorageResult<Vec<KnowledgeSearchHit>> {
        let top_k = query.top_k.max(1);
        let candidate_k = (top_k * 3).max(15);

        let query_vec = if let Some(v) = &query.query_vector {
            v.clone()
        } else {
            self.embedder
                .embed_one(&query.query_text)
                .await
                .map_err(|e| StorageError::BackendUnavailable(format!("embedding failed: {e}")))?
        };

        let filter = KnowledgeVectorFilter {
            target_population: query.target_population.clone(),
            department: query.department.clone(),
            tenant_namespace: query.tenant_namespace.clone(),
        };

        let mut bm25_query = query.clone();
        bm25_query.top_k = candidate_k;
        let tantivy = Arc::clone(&self.tantivy);

        // Execute vector search and blocking Tantivy BM25 search concurrently without starving Tokio workers
        let (vector_res, bm25_join_res) = tokio::join!(
            self.vector_store.search_vectors(&query_vec, &filter, candidate_k),
            tokio::task::spawn_blocking(move || tantivy.search_bm25(&bm25_query))
        );
        let vector_hits = vector_res?;
        let bm25_hits = bm25_join_res
            .map_err(|e| StorageError::VectorStore(format!("tantivy blocking join error: {e}")))?
            .map_err(|e| StorageError::VectorStore(e.to_string()))?;

        // Reciprocal Rank Fusion (RRF)
        let mut fused: HashMap<String, KnowledgeSearchHit> = HashMap::new();
        for (rank, mut hit) in vector_hits.into_iter().enumerate() {
            let rrf_add = 1.0 / (self.rrf_k + (rank as f32) + 1.0);
            hit.rrf_score = rrf_add;
            fused.insert(hit.doc_id.clone(), hit);
        }
        for (rank, mut hit) in bm25_hits.into_iter().enumerate() {
            let rrf_add = 1.0 / (self.rrf_k + (rank as f32) + 1.0);
            if let Some(existing) = fused.get_mut(&hit.doc_id) {
                existing.bm25_score = hit.bm25_score;
                existing.rrf_score += rrf_add;
            } else {
                hit.rrf_score = rrf_add;
                fused.insert(hit.doc_id.clone(), hit);
            }
        }

        let mut results: Vec<KnowledgeSearchHit> = fused.into_values().collect();
        // Slight authority_level tie-breaker weight while preserving RRF ordering
        for hit in &mut results {
            hit.rrf_score *= 0.9 + 0.1 * hit.authority_level.clamp(0.1, 1.0);
        }
        results.sort_by(|a, b| {
            b.rrf_score
                .partial_cmp(&a.rrf_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Optional external Reranker
        if let Some(reranker) = &self.reranker
            && !results.is_empty()
        {
            let candidates: Vec<RerankCandidate> = results
                .iter()
                .map(|h| RerankCandidate {
                    doc_id: h.doc_id.clone(),
                    text: h.content.clone(),
                    initial_score: h.rrf_score,
                })
                .collect();
            if let Ok(scores) = reranker.rerank(&query.query_text, &candidates).await
                && scores.len() == results.len()
            {
                for (hit, score) in results.iter_mut().zip(scores.into_iter()) {
                    hit.rrf_score = score;
                }
                results.sort_by(|a, b| {
                    b.rrf_score
                        .partial_cmp(&a.rrf_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }

        results.truncate(top_k);
        Ok(results)
    }
}

/// Optional local ONNX embedding provider backed by `fastembed-rs` (enabled via `local-fastembed` feature).
#[cfg(feature = "local-fastembed")]
pub struct LocalFastEmbedProvider {
    id: String,
    model: Mutex<fastembed::TextEmbedding>,
    dim: usize,
}

#[cfg(feature = "local-fastembed")]
impl LocalFastEmbedProvider {
    pub fn try_new(id: impl Into<String>) -> PipelineResult<Self> {
        let model = fastembed::TextEmbedding::try_new(Default::default())
            .map_err(|e| PipelineError::Index(format!("fastembed init error: {e}")))?;
        Ok(Self {
            id: id.into(),
            model: Mutex::new(model),
            dim: 1024,
        })
    }
}

#[cfg(feature = "local-fastembed")]
#[async_trait]
impl EmbeddingProvider for LocalFastEmbedProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    async fn embed_batch(
        &self,
        texts: &[String],
    ) -> medical_core::llm::LlmResult<Vec<Vec<f32>>> {
        let guard = self.model.lock().map_err(|_| {
            medical_core::llm::LlmError::InvalidResponse {
                provider: self.id.clone(),
                message: "fastembed mutex poisoned".to_string(),
            }
        })?;
        guard.embed(texts.to_vec(), None).map_err(|e| {
            medical_core::llm::LlmError::InvalidResponse {
                provider: self.id.clone(),
                message: e.to_string(),
            }
        })
    }
}
