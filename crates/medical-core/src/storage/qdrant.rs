use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use qdrant_client::qdrant::value::Kind;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, CreateFieldIndexCollectionBuilder, DeletePointsBuilder,
    Distance, FieldType, Filter, HnswConfigDiffBuilder, KeywordIndexParamsBuilder, PointStruct,
    QueryPointsBuilder, ScoredPoint, Value, VectorParamsBuilder, payload_index_params::IndexParams,
};
use qdrant_client::{Payload, Qdrant};
use uuid::Uuid;

use crate::models::{
    L3VisitEvent, RecalledL3Event, SectionType, SourceType, SuggestedFollowUpSlot,
    TargetPopulation,
};

use super::traits::{
    KnowledgeSearchHit, KnowledgeVectorFilter, KnowledgeVectorStore, PatientL3VectorStore,
    VectorizedKnowledgeChunk,
};
use super::{StorageError, StorageResult};

pub const DEFAULT_KNOWLEDGE_COLLECTION: &str = "medical_knowledge";
pub const DEFAULT_PATIENT_L3_COLLECTION: &str = "patient_l3_events";

pub fn connect_qdrant(url: &str, api_key: Option<&str>) -> StorageResult<Arc<Qdrant>> {
    let mut builder = Qdrant::from_url(url);
    if let Some(key) = api_key
        && !key.trim().is_empty()
    {
        builder = builder.api_key(key.to_string());
    }
    let client = builder.build()?;
    Ok(Arc::new(client))
}

fn deterministic_point_uuid(raw_id: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, raw_id.as_bytes()).to_string()
}

fn payload_str(payload: &HashMap<String, Value>, key: &str) -> String {
    payload
        .get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            Kind::StringValue(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn payload_f32(payload: &HashMap<String, Value>, key: &str) -> Option<f32> {
    payload.get(key).and_then(|v| v.kind.as_ref()).and_then(|k| match k {
        Kind::DoubleValue(d) => Some(*d as f32),
        Kind::IntegerValue(i) => Some(*i as f32),
        _ => None,
    })
}

fn payload_i64(payload: &HashMap<String, Value>, key: &str) -> Option<i64> {
    payload.get(key).and_then(|v| v.kind.as_ref()).and_then(|k| match k {
        Kind::IntegerValue(i) => Some(*i),
        Kind::DoubleValue(d) => Some(*d as i64),
        _ => None,
    })
}

fn payload_string_list(payload: &HashMap<String, Value>, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            Kind::ListValue(list) => Some(
                list.values
                    .iter()
                    .filter_map(|item| match item.kind.as_ref() {
                        Some(Kind::StringValue(s)) => Some(s.clone()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn payload_json_deserialize<T: serde::de::DeserializeOwned>(
    payload: &HashMap<String, Value>,
    key: &str,
) -> Option<T> {
    let raw = payload_str(payload, key);
    if raw.is_empty() {
        return None;
    }
    serde_json::from_str(&raw).ok()
}

#[derive(Clone)]
pub struct QdrantKnowledgeStore {
    client: Arc<Qdrant>,
    collection: String,
    tenant_namespace: Option<String>,
}

impl QdrantKnowledgeStore {
    pub fn new(client: Arc<Qdrant>) -> Self {
        Self {
            client,
            collection: DEFAULT_KNOWLEDGE_COLLECTION.to_string(),
            tenant_namespace: None,
        }
    }

    pub fn with_collection(mut self, collection: impl Into<String>) -> Self {
        self.collection = collection.into();
        self
    }

    pub fn with_tenant(mut self, tenant_namespace: impl Into<String>) -> Self {
        self.tenant_namespace = Some(tenant_namespace.into());
        self
    }

    fn point_to_hit(point: ScoredPoint) -> KnowledgeSearchHit {
        let p = &point.payload;
        let source_type: SourceType =
            payload_json_deserialize(p, "source_type_json").unwrap_or(SourceType::Textbook);
        let section_type: SectionType = payload_json_deserialize(p, "section_type_json")
            .unwrap_or(SectionType::ClinicalManifestation);
        let target_population: Vec<TargetPopulation> =
            payload_json_deserialize(p, "target_population_json").unwrap_or_default();
        let suggested_follow_up_slots: Vec<SuggestedFollowUpSlot> =
            payload_json_deserialize(p, "suggested_follow_up_slots_json").unwrap_or_default();
        let ns = payload_str(p, "tenant_namespace");

        KnowledgeSearchHit {
            doc_id: payload_str(p, "doc_id"),
            source_title: payload_str(p, "source_title"),
            source_type,
            authority_level: payload_f32(p, "authority_level").unwrap_or(0.8),
            disease_name: payload_str(p, "disease_name"),
            department: payload_str(p, "department"),
            secondary_departments: payload_string_list(p, "secondary_departments"),
            target_population,
            section_type,
            content: payload_str(p, "content"),
            key_symptoms: payload_string_list(p, "key_symptoms"),
            red_flag_signs: payload_string_list(p, "red_flag_signs"),
            seasonal_tags: payload_string_list(p, "seasonal_tags"),
            suggested_follow_up_slots,
            vector_score: Some(point.score),
            bm25_score: None,
            rrf_score: point.score,
            tenant_namespace: if ns.is_empty() { None } else { Some(ns) },
        }
    }
}

#[async_trait]
impl KnowledgeVectorStore for QdrantKnowledgeStore {
    async fn ensure_collection(&self, vector_dim: u64) -> StorageResult<()> {
        if !self.client.collection_exists(&self.collection).await? {
            self.client
                .create_collection(
                    CreateCollectionBuilder::new(&self.collection)
                        .vectors_config(VectorParamsBuilder::new(vector_dim, Distance::Cosine))
                        .hnsw_config(HnswConfigDiffBuilder::default().m(16).ef_construct(128)),
                )
                .await?;
        }
        let indexes = [
            ("tenant_namespace", FieldType::Keyword, true),
            ("doc_id", FieldType::Keyword, false),
            ("target_population", FieldType::Keyword, false),
            ("department", FieldType::Keyword, false),
            ("secondary_departments", FieldType::Keyword, false),
            ("authority_level", FieldType::Float, false),
            ("seasonal_tags", FieldType::Keyword, false),
            ("section_type", FieldType::Keyword, false),
            ("disease_name", FieldType::Keyword, false),
        ];
        for (field, ftype, is_tenant) in indexes {
            let mut builder =
                CreateFieldIndexCollectionBuilder::new(&self.collection, field, ftype).wait(true);
            if is_tenant {
                builder = builder.field_index_params(IndexParams::KeywordIndexParams(
                    KeywordIndexParamsBuilder::default().is_tenant(true).build(),
                ));
            }
            self.client.create_field_index(builder).await?;
        }
        Ok(())
    }

    async fn upsert_documents(&self, chunks: &[VectorizedKnowledgeChunk]) -> StorageResult<u64> {
        if chunks.is_empty() {
            return Ok(0);
        }
        let mut points = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let d = &chunk.document;
            let pop_strings: Vec<&'static str> =
                d.target_population.iter().map(|p| p.as_str()).collect();
            let payload_json = serde_json::json!({
                "tenant_namespace": chunk.tenant_namespace,
                "doc_id": d.doc_id,
                "source_title": d.source_title,
                "source_type_json": serde_json::to_string(&d.source_type)?,
                "authority_level": d.authority_level,
                "disease_name": d.disease_name,
                "department": d.department,
                "secondary_departments": d.secondary_departments,
                "target_population": pop_strings,
                "target_population_json": serde_json::to_string(&d.target_population)?,
                "section_type": d.section_type.as_str(),
                "section_type_json": serde_json::to_string(&d.section_type)?,
                "content": d.content,
                "key_symptoms": d.key_symptoms,
                "red_flag_signs": d.red_flag_signs,
                "seasonal_tags": d.seasonal_tags,
                "suggested_follow_up_slots_json": serde_json::to_string(&d.suggested_follow_up_slots)?,
            });
            let payload = Payload::try_from(payload_json)
                .map_err(|e| StorageError::VectorStore(e.to_string()))?;
            let point_id = deterministic_point_uuid(&format!("{}:{}", chunk.tenant_namespace, d.doc_id));
            points.push(PointStruct::new(point_id, chunk.vector.clone(), payload));
        }

        let count = points.len() as u64;
        self.client
            .upsert_points(qdrant_client::qdrant::UpsertPointsBuilder::new(
                &self.collection,
                points,
            ).wait(true))
            .await?;
        Ok(count)
    }

    async fn search_vectors(
        &self,
        vector: &[f32],
        filter: &KnowledgeVectorFilter,
        top_k: usize,
    ) -> StorageResult<Vec<KnowledgeSearchHit>> {
        let mut conditions = Vec::new();
        if let Some(ns) = &filter.tenant_namespace {
            let ns_trimmed = ns.trim();
            let effective_ns = if ns_trimmed.is_empty() {
                "default"
            } else {
                ns_trimmed
            };
            let allowed = if effective_ns == "default" {
                vec!["default".to_string()]
            } else {
                vec![effective_ns.to_string(), "default".to_string()]
            };
            conditions.push(Condition::matches("tenant_namespace", allowed));
        }
        if let Some(dept) = &filter.department {
            conditions.push(Condition::matches("department", dept.clone()));
        }
        if let Some(pops) = &filter.target_population
            && !pops.is_empty()
        {
            let strs: Vec<String> = pops.iter().map(|p| p.as_str().to_string()).collect();
            conditions.push(Condition::matches("target_population", strs));
        }

        let mut builder = QueryPointsBuilder::new(&self.collection)
            .query(vector.to_vec())
            .limit(top_k.max(1) as u64)
            .with_payload(true);
        if !conditions.is_empty() {
            builder = builder.filter(Filter::must(conditions));
        }

        let response = self.client.query(builder).await?;
        Ok(response.result.into_iter().map(Self::point_to_hit).collect())
    }

    async fn delete_by_doc_ids(&self, doc_ids: &[String]) -> StorageResult<u64> {
        if doc_ids.is_empty() {
            return Ok(0);
        }
        let mut conditions = vec![Condition::matches("doc_id", doc_ids.to_vec())];
        if let Some(ns) = &self.tenant_namespace {
            let ns_trimmed = ns.trim();
            let effective_ns = if ns_trimmed.is_empty() {
                "default"
            } else {
                ns_trimmed
            };
            conditions.push(Condition::matches(
                "tenant_namespace",
                effective_ns.to_string(),
            ));
        }
        let filter = Filter::must(conditions);
        let count = doc_ids.len() as u64;
        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection)
                    .points(filter)
                    .wait(true),
            )
            .await?;
        Ok(count)
    }
}

#[derive(Clone)]
pub struct QdrantPatientL3Store {
    client: Arc<Qdrant>,
    collection: String,
    tenant_id: String,
}

impl QdrantPatientL3Store {
    pub fn new(client: Arc<Qdrant>) -> Self {
        Self {
            client,
            collection: DEFAULT_PATIENT_L3_COLLECTION.to_string(),
            tenant_id: "default".to_string(),
        }
    }

    pub fn with_collection(mut self, collection: impl Into<String>) -> Self {
        self.collection = collection.into();
        self
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }
}

#[async_trait]
impl PatientL3VectorStore for QdrantPatientL3Store {
    async fn ensure_collection(&self, vector_dim: u64) -> StorageResult<()> {
        if !self.client.collection_exists(&self.collection).await? {
            self.client
                .create_collection(
                    CreateCollectionBuilder::new(&self.collection)
                        .vectors_config(VectorParamsBuilder::new(vector_dim, Distance::Cosine))
                        .hnsw_config(HnswConfigDiffBuilder::default().m(16).ef_construct(128)),
                )
                .await?;
        }
        let indexes = [
            ("tenant_id", FieldType::Keyword, true),
            ("patient_id", FieldType::Keyword, false),
            ("event_id", FieldType::Keyword, false),
            ("recommended_department", FieldType::Keyword, false),
            ("urgency_level", FieldType::Integer, false),
            ("occurred_at_unix", FieldType::Integer, false),
        ];
        for (field, ftype, is_tenant) in indexes {
            let mut builder =
                CreateFieldIndexCollectionBuilder::new(&self.collection, field, ftype).wait(true);
            if is_tenant {
                builder = builder.field_index_params(IndexParams::KeywordIndexParams(
                    KeywordIndexParamsBuilder::default().is_tenant(true).build(),
                ));
            }
            self.client.create_field_index(builder).await?;
        }
        Ok(())
    }

    async fn upsert_event_vector(
        &self,
        patient_id: &str,
        event: &L3VisitEvent,
        vector: Vec<f32>,
    ) -> StorageResult<()> {
        // Store PII-scrubbed metadata fields in vector payload; full encrypted text resides in PG.
        let scrubbed_summary = crate::privacy::scrub_pii(&event.summary).scrubbed_text;
        let scrubbed_chief = crate::privacy::scrub_pii(&event.chief_complaint).scrubbed_text;
        let payload_json = serde_json::json!({
            "tenant_id": self.tenant_id,
            "patient_id": patient_id,
            "event_id": event.event_id,
            "recommended_department": event.recommended_department,
            "urgency_level": event.urgency_level as i64,
            "occurred_at_unix": event.occurred_at.timestamp(),
            "extracted_symptoms": event.extracted_symptoms,
            "summary_preview": scrubbed_summary,
            "chief_complaint_preview": scrubbed_chief,
        });
        let payload = Payload::try_from(payload_json)
            .map_err(|e| StorageError::VectorStore(e.to_string()))?;
        let point_id = deterministic_point_uuid(&format!("{}:{}:{}", self.tenant_id, patient_id, event.event_id));
        let point = PointStruct::new(point_id, vector, payload);

        self.client
            .upsert_points(
                qdrant_client::qdrant::UpsertPointsBuilder::new(&self.collection, vec![point])
                    .wait(true),
            )
            .await?;
        Ok(())
    }

    async fn recall_similar_events(
        &self,
        patient_id: &str,
        query_vector: &[f32],
        top_k: usize,
    ) -> StorageResult<Vec<RecalledL3Event>> {
        let filter = Filter::must([
            Condition::matches("tenant_id", self.tenant_id.clone()),
            Condition::matches("patient_id", patient_id.to_string()),
        ]);
        let response = self
            .client
            .query(
                QueryPointsBuilder::new(&self.collection)
                    .query(query_vector.to_vec())
                    .filter(filter)
                    .limit(top_k.max(1) as u64)
                    .with_payload(true),
            )
            .await?;

        Ok(response
            .result
            .into_iter()
            .map(|pt| {
                let p = &pt.payload;
                let ts = payload_i64(p, "occurred_at_unix").unwrap_or(0);
                let occurred_at = DateTime::<Utc>::from_timestamp(ts, 0).unwrap_or_else(Utc::now);
                RecalledL3Event {
                    event: L3VisitEvent {
                        event_id: payload_str(p, "event_id"),
                        occurred_at,
                        chief_complaint: payload_str(p, "chief_complaint_preview"),
                        extracted_symptoms: payload_string_list(p, "extracted_symptoms"),
                        recommended_department: payload_str(p, "recommended_department"),
                        urgency_level: payload_i64(p, "urgency_level").unwrap_or(2).clamp(1, 4) as u8,
                        summary: payload_str(p, "summary_preview"),
                        clinical_exams: Default::default(),
                    },
                    similarity_score: pt.score,
                }
            })
            .collect())
    }

    async fn delete_patient_events(&self, patient_id: &str) -> StorageResult<u64> {
        let filter = Filter::must([
            Condition::matches("tenant_id", self.tenant_id.clone()),
            Condition::matches("patient_id", patient_id.to_string()),
        ]);
        self.client
            .delete_points(
                DeletePointsBuilder::new(&self.collection)
                    .points(filter)
                    .wait(true),
            )
            .await?;
        Ok(1)
    }
}
