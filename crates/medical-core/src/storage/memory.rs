use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use rand::RngCore;
use uuid::Uuid;

use crate::crypto::{EncryptedEnvelope, FieldCipher, hash_api_key};
use crate::llm::{ProviderConfig, RouterConfig};
use crate::models::{
    EpidemicContext, Gender, KnowledgeDocument, L2Profile, L2SensitivePayload, L3SensitivePayload,
    L3VisitEvent, MedicalDictionaries, PatientRecord, RecalledL3Event, RedFlagLevel, RedFlagRule,
    SymptomSynonym,
};

use super::traits::{
    ApiKeyRecord, ApiKeyStore, AuditLogStore, CheckpointStore, DictionaryStore, EpidemicStore,
    GatewayMetricsSummary, IngestionCheckpoint, IssueApiKeyRequest, IssuedApiKey,
    KnowledgeSearchEngine, KnowledgeSearchHit, KnowledgeSearchQuery, KnowledgeVectorFilter,
    KnowledgeVectorStore, LlmProviderStore, PatientL2Record, PatientL3VectorStore,
    PatientMemoryStore, ProviderUsageStat, SessionCache, SessionState, TriageAuditLog,
    VectorizedKnowledgeChunk, WipedMemoryStats,
};
use super::{StorageError, StorageResult};

#[derive(Debug, Clone)]
pub struct EncryptedL2Row {
    pub patient_id: String,
    pub consent_long_term_memory: bool,
    pub age_group: String,
    pub gender: Gender,
    pub is_pregnant: bool,
    pub encrypted_payload: EncryptedEnvelope,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct EncryptedL3Row {
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub extracted_symptoms: Vec<String>,
    pub recommended_department: String,
    pub urgency_level: u8,
    pub encrypted_payload: EncryptedEnvelope,
}

pub struct InMemoryPatientMemoryStore {
    cipher: Arc<FieldCipher>,
    l2: DashMap<String, EncryptedL2Row>,
    l3: DashMap<String, Vec<EncryptedL3Row>>,
}

impl InMemoryPatientMemoryStore {
    pub fn new(cipher: Arc<FieldCipher>) -> Self {
        Self {
            cipher,
            l2: DashMap::new(),
            l3: DashMap::new(),
        }
    }

    /// Test inspection helper verifying sensitive fields are AES-256-GCM encrypted at rest.
    pub fn raw_encrypted_l2(&self, patient_id: &str) -> Option<EncryptedL2Row> {
        self.l2.get(patient_id).map(|r| r.value().clone())
    }

    pub fn raw_encrypted_l3(&self, patient_id: &str) -> Vec<EncryptedL3Row> {
        self.l3
            .get(patient_id)
            .map(|r| r.value().clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl PatientMemoryStore for InMemoryPatientMemoryStore {
    async fn get_l2_profile(&self, patient_id: &str) -> StorageResult<Option<PatientL2Record>> {
        let Some(row) = self.l2.get(patient_id).map(|r| r.value().clone()) else {
            return Ok(None);
        };
        let sensitive: L2SensitivePayload = self.cipher.decrypt_json(&row.encrypted_payload)?;
        Ok(Some(PatientL2Record {
            patient_id: row.patient_id,
            consent_long_term_memory: row.consent_long_term_memory,
            l2_profile: L2Profile {
                age_group: row.age_group,
                gender: row.gender,
                is_pregnant: row.is_pregnant,
                chronic_diseases: sensitive.chronic_diseases,
                allergies: sensitive.allergies,
                surgical_history: sensitive.surgical_history,
                long_term_medications: sensitive.long_term_medications,
            },
            updated_at: row.updated_at,
        }))
    }

    async fn upsert_l2_profile(
        &self,
        patient_id: &str,
        consent: bool,
        profile: &L2Profile,
    ) -> StorageResult<()> {
        let sensitive = L2SensitivePayload::from(profile);
        let encrypted_payload = self.cipher.encrypt_json(&sensitive)?;
        self.l2.insert(
            patient_id.to_string(),
            EncryptedL2Row {
                patient_id: patient_id.to_string(),
                consent_long_term_memory: consent,
                age_group: profile.age_group.clone(),
                gender: profile.gender,
                is_pregnant: profile.is_pregnant,
                encrypted_payload,
                updated_at: Utc::now(),
            },
        );
        Ok(())
    }

    async fn insert_l3_event(&self, patient_id: &str, event: &L3VisitEvent) -> StorageResult<()> {
        let sensitive = L3SensitivePayload::from(event);
        let encrypted_payload = self.cipher.encrypt_json(&sensitive)?;
        let row = EncryptedL3Row {
            event_id: event.event_id.clone(),
            occurred_at: event.occurred_at,
            extracted_symptoms: event.extracted_symptoms.clone(),
            recommended_department: event.recommended_department.clone(),
            urgency_level: event.urgency_level,
            encrypted_payload,
        };
        let mut entry = self.l3.entry(patient_id.to_string()).or_default();
        if let Some(existing) = entry.iter_mut().find(|e| e.event_id == row.event_id) {
            *existing = row;
        } else {
            entry.push(row);
        }
        Ok(())
    }

    async fn list_l3_events(&self, patient_id: &str, limit: usize) -> StorageResult<Vec<L3VisitEvent>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut rows = self.raw_encrypted_l3(patient_id);
        rows.sort_by_key(|a| std::cmp::Reverse(a.occurred_at));
        rows.truncate(limit);
        rows.into_iter()
            .map(|r| {
                let sensitive: L3SensitivePayload =
                    self.cipher.decrypt_json(&r.encrypted_payload)?;
                Ok(L3VisitEvent {
                    event_id: r.event_id,
                    occurred_at: r.occurred_at,
                    chief_complaint: sensitive.chief_complaint,
                    extracted_symptoms: r.extracted_symptoms,
                    recommended_department: r.recommended_department,
                    urgency_level: r.urgency_level,
                    summary: sensitive.summary,
                })
            })
            .collect()
    }

    async fn get_full_memory(
        &self,
        patient_id: &str,
        l3_limit: usize,
    ) -> StorageResult<Option<PatientRecord>> {
        let Some(l2) = self.get_l2_profile(patient_id).await? else {
            return Ok(None);
        };
        let events = self.list_l3_events(patient_id, l3_limit).await?;
        Ok(Some(PatientRecord {
            patient_id: l2.patient_id,
            consent_long_term_memory: l2.consent_long_term_memory,
            l2_profile: l2.l2_profile,
            l3_visit_events: events,
        }))
    }

    async fn list_patient_ids(&self, limit: usize, offset: usize) -> StorageResult<Vec<String>> {
        let mut items: Vec<(String, DateTime<Utc>)> = self
            .l2
            .iter()
            .map(|e| (e.key().clone(), e.value().updated_at))
            .collect();
        items.sort_by_key(|a| std::cmp::Reverse(a.1));
        Ok(items
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(id, _)| id)
            .collect())
    }

    async fn delete_patient_memory(&self, patient_id: &str) -> StorageResult<WipedMemoryStats> {
        let l2_del = u64::from(self.l2.remove(patient_id).is_some());
        let l3_del = self
            .l3
            .remove(patient_id)
            .map(|(_, v)| v.len() as u64)
            .unwrap_or(0);
        Ok(WipedMemoryStats {
            l2_profiles_deleted: l2_del,
            l3_events_deleted: l3_del,
            qdrant_points_deleted: 0,
            l1_sessions_deleted: 0,
        })
    }
}

#[derive(Default)]
pub struct InMemoryPatientL3VectorStore {
    events: DashMap<String, Vec<(L3VisitEvent, Vec<f32>)>>,
}

impl InMemoryPatientL3VectorStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl PatientL3VectorStore for InMemoryPatientL3VectorStore {
    async fn ensure_collection(&self, _vector_dim: u64) -> StorageResult<()> {
        Ok(())
    }

    async fn upsert_event_vector(
        &self,
        patient_id: &str,
        event: &L3VisitEvent,
        vector: Vec<f32>,
    ) -> StorageResult<()> {
        let mut entry = self.events.entry(patient_id.to_string()).or_default();
        if let Some(pos) = entry.iter_mut().find(|(e, _)| e.event_id == event.event_id) {
            *pos = (event.clone(), vector);
        } else {
            entry.push((event.clone(), vector));
        }
        Ok(())
    }

    async fn recall_similar_events(
        &self,
        patient_id: &str,
        query_vector: &[f32],
        top_k: usize,
    ) -> StorageResult<Vec<RecalledL3Event>> {
        let Some(entries) = self.events.get(patient_id) else {
            return Ok(Vec::new());
        };
        let mut scored: Vec<RecalledL3Event> = entries
            .iter()
            .map(|(event, vec)| RecalledL3Event {
                event: event.clone(),
                similarity_score: cosine_similarity(query_vector, vec),
            })
            .collect();
        scored.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(top_k);
        Ok(scored)
    }

    async fn delete_patient_events(&self, patient_id: &str) -> StorageResult<u64> {
        Ok(self
            .events
            .remove(patient_id)
            .map(|(_, v)| v.len() as u64)
            .unwrap_or(0))
    }
}

#[derive(Default)]
pub struct InMemoryEpidemicStore {
    contexts: DashMap<String, EpidemicContext>,
}

impl InMemoryEpidemicStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl EpidemicStore for InMemoryEpidemicStore {
    async fn list_active(
        &self,
        region_code: Option<&str>,
        season: Option<&str>,
        as_of: NaiveDate,
    ) -> StorageResult<Vec<EpidemicContext>> {
        let mut out: Vec<EpidemicContext> = self
            .contexts
            .iter()
            .map(|e| e.value().clone())
            .filter(|c| {
                c.active
                    && c.valid_from <= as_of
                    && c.valid_to >= as_of
                    && region_code.map(|r| c.region_code == r).unwrap_or(true)
                    && season
                        .map(|s| c.season.as_str() == s || c.season.as_str() == "all_year")
                        .unwrap_or(true)
            })
            .collect();
        out.sort_by(|a, b| {
            b.intensity
                .partial_cmp(&a.intensity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }

    async fn list_all(&self) -> StorageResult<Vec<EpidemicContext>> {
        Ok(self.contexts.iter().map(|e| e.value().clone()).collect())
    }

    async fn upsert(&self, ctx: &EpidemicContext) -> StorageResult<()> {
        self.contexts.insert(ctx.context_id.clone(), ctx.clone());
        Ok(())
    }

    async fn delete(&self, context_id: &str) -> StorageResult<bool> {
        Ok(self.contexts.remove(context_id).is_some())
    }
}

#[derive(Default)]
pub struct InMemoryDictionaryStore {
    synonyms: DashMap<String, SymptomSynonym>,
    red_flags: DashMap<String, RedFlagRule>,
}

impl InMemoryDictionaryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_defaults() -> Self {
        let store = Self::new();
        let default_synonyms = vec![
            SymptomSynonym {
                standard_term: "上腹痛".to_string(),
                colloquial_terms: vec![
                    "心口窝疼".to_string(),
                    "胃口疼".to_string(),
                    "上肚子痛".to_string(),
                    "剑突下疼".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "呼吸困难".to_string(),
                colloquial_terms: vec![
                    "喘不上气".to_string(),
                    "喘不过气".to_string(),
                    "气短".to_string(),
                    "憋气".to_string(),
                    "吸不进气".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "胸痛".to_string(),
                colloquial_terms: vec![
                    "胸口疼".to_string(),
                    "胸口痛".to_string(),
                    "胸口闷痛".to_string(),
                    "胸口有点闷痛".to_string(),
                    "胸闷痛".to_string(),
                    "胸部闷痛".to_string(),
                    "胸口撕裂样疼".to_string(),
                    "心口窝撕裂样疼".to_string(),
                    "心前区痛".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "左臂放射痛".to_string(),
                colloquial_terms: vec![
                    "左臂发麻放射痛".to_string(),
                    "左肩臂放射痛".to_string(),
                    "左手臂放射痛".to_string(),
                    "左臂放射性疼痛".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "大汗".to_string(),
                colloquial_terms: vec![
                    "出冷汗".to_string(),
                    "大汗淋漓".to_string(),
                    "冒冷汗".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "水样便".to_string(),
                colloquial_terms: vec!["拉水".to_string(), "拉稀".to_string()],
            },
            SymptomSynonym {
                standard_term: "发热".to_string(),
                colloquial_terms: vec![
                    "发烧".to_string(),
                    "高烧".to_string(),
                    "低烧".to_string(),
                    "烧起来".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "咳嗽".to_string(),
                colloquial_terms: vec![
                    "干咳".to_string(),
                    "咳痰".to_string(),
                    "老咳".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "腹痛".to_string(),
                colloquial_terms: vec![
                    "肚子疼".to_string(),
                    "肚子痛".to_string(),
                    "胃痛".to_string(),
                    "胃疼".to_string(),
                ],
            },
            SymptomSynonym {
                standard_term: "头痛".to_string(),
                colloquial_terms: vec!["头疼".to_string(), "偏头痛".to_string()],
            },
            SymptomSynonym {
                standard_term: "腹泻".to_string(),
                colloquial_terms: vec!["拉肚子".to_string()],
            },
            SymptomSynonym {
                standard_term: "呕吐".to_string(),
                colloquial_terms: vec!["吐了".to_string(), "干呕".to_string()],
            },
        ];
        for s in default_synonyms {
            store.synonyms.insert(s.standard_term.clone(), s);
        }

        let default_rules = vec![
            RedFlagRule {
                rule_id: "rf_acute_chest_pain".to_string(),
                level: RedFlagLevel::Emergency120,
                trigger_all_of: vec!["胸痛".to_string()],
                trigger_any_of: vec![
                    "大汗".to_string(),
                    "左臂放射痛".to_string(),
                    "呼吸困难".to_string(),
                    "濒死感".to_string(),
                    "撕裂样".to_string(),
                ],
                alert_message: "您描述的症状提示可能存在急性心血管或胸部危重症风险，请立即停止等待，尽快拨打 120 或前往最近医院急诊科就诊！".to_string(),
            },
            RedFlagRule {
                rule_id: "rf_acute_hemoptysis_coma".to_string(),
                level: RedFlagLevel::Emergency120,
                trigger_all_of: vec![],
                trigger_any_of: vec![
                    "昏迷".to_string(),
                    "意识丧失".to_string(),
                    "大咯血".to_string(),
                    "口唇发绀".to_string(),
                ],
                alert_message: "检测到危急重症预警信号，请立即拨打 120 急救电话或就近急诊抢救！".to_string(),
            },
            RedFlagRule {
                rule_id: "rf_acute_stroke".to_string(),
                level: RedFlagLevel::Emergency120,
                trigger_all_of: vec![],
                trigger_any_of: vec![
                    "偏瘫".to_string(),
                    "口角歪斜".to_string(),
                    "言语不清".to_string(),
                    "说话不清".to_string(),
                    "一侧肢体麻木".to_string(),
                    "一侧肢体无力".to_string(),
                    "半身不遂".to_string(),
                    "突发失语".to_string(),
                ],
                alert_message: "您描述的症状高度提示急性脑卒中（中风）风险，黄金溶栓时间窗极其宝贵，请立即拨打 120 急救电话或火速前往具备卒中中心的医院急诊科！".to_string(),
            },
            RedFlagRule {
                rule_id: "rf_suicide_self_harm".to_string(),
                level: RedFlagLevel::Emergency120,
                trigger_all_of: vec![],
                trigger_any_of: vec![
                    "自杀".to_string(),
                    "轻生".to_string(),
                    "割腕".to_string(),
                    "服毒".to_string(),
                    "过量服药".to_string(),
                    "跳楼".to_string(),
                    "吞安眠药".to_string(),
                    "吞服安眠药".to_string(),
                ],
                alert_message: "检测到严重自伤/服毒危急风险！如已服药或受伤请立即拨打 120 急救并告知身边亲友；如需紧急心理危机干预，请立即拨打全国心理援助热线 12356 或 400-161-9995。".to_string(),
            },
            RedFlagRule {
                rule_id: "rf_acute_gi_bleed".to_string(),
                level: RedFlagLevel::Emergency120,
                trigger_all_of: vec![],
                trigger_any_of: vec![
                    "大量呕血".to_string(),
                    "呕血".to_string(),
                    "大量便血".to_string(),
                    "呕血黑便".to_string(),
                    "失血性休克".to_string(),
                    "休克".to_string(),
                    "大出血".to_string(),
                ],
                alert_message: "您描述的症状高度提示急性消化道大出血或失血性休克风险，请立即禁食禁水并拨打 120 急救电话或就近急诊抢救！".to_string(),
            },
            RedFlagRule {
                rule_id: "rf_anaphylaxis_dyspnea".to_string(),
                level: RedFlagLevel::Emergency120,
                trigger_all_of: vec![],
                trigger_any_of: vec![
                    "过敏性休克".to_string(),
                    "喉头水肿".to_string(),
                    "严重呼吸困难".to_string(),
                    "喘不上气".to_string(),
                    "窒息感".to_string(),
                    "呼吸窘迫".to_string(),
                ],
                alert_message: "检测到急性气道梗阻、严重呼吸困难或过敏性休克风险，请立即保持气道通畅并拨打 120 急救电话！".to_string(),
            },
            RedFlagRule {
                rule_id: "rf_obstetric_pediatric_emergency".to_string(),
                level: RedFlagLevel::Emergency120,
                trigger_all_of: vec![],
                trigger_any_of: vec![
                    "孕妇大出血".to_string(),
                    "胎动消失".to_string(),
                    "高热惊厥".to_string(),
                    "全身抽搐".to_string(),
                    "持续抽搐".to_string(),
                ],
                alert_message: "检测到产科或儿科危急重症信号，请立即拨打 120 急救电话或立即前往医院急诊抢救！".to_string(),
            },
            RedFlagRule {
                rule_id: "rf_urgent_high_risk".to_string(),
                level: RedFlagLevel::Urgent,
                trigger_all_of: vec![],
                trigger_any_of: vec![
                    "持续高热不退".to_string(),
                    "妊娠期阴道出血".to_string(),
                    "孕妇腹痛出血".to_string(),
                ],
                alert_message: "您描述的症状属于高危紧急情况，建议立即前往医院急诊科或对应专科急诊评估，切勿拖延等待！".to_string(),
            },
        ];
        for r in default_rules {
            store.red_flags.insert(r.rule_id.clone(), r);
        }
        store
    }
}

#[async_trait]
impl DictionaryStore for InMemoryDictionaryStore {
    async fn load_dictionaries(&self) -> StorageResult<MedicalDictionaries> {
        Ok(MedicalDictionaries {
            symptom_synonyms: self.list_symptom_synonyms().await?,
            red_flag_rules: self.list_red_flag_rules().await?,
        })
    }

    async fn list_symptom_synonyms(&self) -> StorageResult<Vec<SymptomSynonym>> {
        let mut list: Vec<SymptomSynonym> =
            self.synonyms.iter().map(|e| e.value().clone()).collect();
        list.sort_by(|a, b| a.standard_term.cmp(&b.standard_term));
        Ok(list)
    }

    async fn save_symptom_synonym(&self, synonym: &SymptomSynonym) -> StorageResult<()> {
        self.synonyms
            .insert(synonym.standard_term.clone(), synonym.clone());
        Ok(())
    }

    async fn delete_symptom_synonym(&self, standard_term: &str) -> StorageResult<bool> {
        Ok(self.synonyms.remove(standard_term).is_some())
    }

    async fn list_red_flag_rules(&self) -> StorageResult<Vec<RedFlagRule>> {
        let mut list: Vec<RedFlagRule> =
            self.red_flags.iter().map(|e| e.value().clone()).collect();
        list.sort_by(|a, b| a.rule_id.cmp(&b.rule_id));
        Ok(list)
    }

    async fn save_red_flag_rule(&self, rule: &RedFlagRule) -> StorageResult<()> {
        self.red_flags.insert(rule.rule_id.clone(), rule.clone());
        Ok(())
    }

    async fn delete_red_flag_rule(&self, rule_id: &str) -> StorageResult<bool> {
        Ok(self.red_flags.remove(rule_id).is_some())
    }
}

#[derive(Default)]
pub struct InMemoryAuditLogStore {
    logs: RwLock<Vec<TriageAuditLog>>,
}

impl InMemoryAuditLogStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl AuditLogStore for InMemoryAuditLogStore {
    async fn append(&self, log: &TriageAuditLog) -> StorageResult<()> {
        self.logs.write().push(log.clone());
        Ok(())
    }

    async fn query_recent(
        &self,
        limit: usize,
        tenant_id: Option<&str>,
    ) -> StorageResult<Vec<TriageAuditLog>> {
        let guard = self.logs.read();
        let mut filtered: Vec<TriageAuditLog> = guard
            .iter()
            .filter(|l| tenant_id.map(|t| l.tenant_id.as_deref() == Some(t)).unwrap_or(true))
            .cloned()
            .collect();
        filtered.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        filtered.truncate(limit);
        Ok(filtered)
    }

    async fn get_metrics_summary(&self, window_hours: u32) -> StorageResult<GatewayMetricsSummary> {
        let cutoff = Utc::now() - chrono::Duration::hours(window_hours.max(1) as i64);
        let guard = self.logs.read();
        let mut total_requests = 0_u64;
        let mut total_errors = 0_u64;
        let mut total_prompt = 0_u64;
        let mut total_completion = 0_u64;
        let mut total_latency = 0_u64;
        let mut by_provider: HashMap<String, (u64, u64, u64, u64, u64)> = HashMap::new();

        for l in guard.iter().filter(|l| l.created_at >= cutoff) {
            total_requests += 1;
            let is_err = l.error_message.is_some();
            if is_err {
                total_errors += 1;
            }
            total_prompt += l.prompt_tokens as u64;
            total_completion += l.completion_tokens as u64;
            total_latency += l.latency_ms;

            let pid = l
                .provider_id
                .clone()
                .unwrap_or_else(|| "local_rule".to_string());
            let e = by_provider.entry(pid).or_insert((0, 0, 0, 0, 0));
            e.0 += 1;
            if is_err {
                e.1 += 1;
            }
            e.2 += l.prompt_tokens as u64;
            e.3 += l.completion_tokens as u64;
            e.4 += l.latency_ms;
        }

        Ok(GatewayMetricsSummary {
            total_requests,
            total_errors,
            error_rate: if total_requests > 0 {
                total_errors as f64 / total_requests as f64
            } else {
                0.0
            },
            total_prompt_tokens: total_prompt,
            total_completion_tokens: total_completion,
            avg_latency_ms: if total_requests > 0 {
                total_latency as f64 / total_requests as f64
            } else {
                0.0
            },
            by_provider: by_provider
                .into_iter()
                .map(|(provider_id, (req, err, pt, ct, lat))| ProviderUsageStat {
                    provider_id,
                    requests: req,
                    errors: err,
                    prompt_tokens: pt,
                    completion_tokens: ct,
                    avg_latency_ms: if req > 0 { lat as f64 / req as f64 } else { 0.0 },
                })
                .collect(),
        })
    }
}

#[derive(Default)]
pub struct InMemoryApiKeyStore {
    by_hash: DashMap<String, ApiKeyRecord>,
    id_to_hash: DashMap<String, String>,
}

impl InMemoryApiKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ApiKeyStore for InMemoryApiKeyStore {
    async fn issue_key(&self, req: IssueApiKeyRequest) -> StorageResult<IssuedApiKey> {
        let mut raw_secret = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut raw_secret);
        let raw_key = format!("mk_live_{}", hex::encode(raw_secret));
        let key_prefix = raw_key.chars().take(16).collect::<String>();
        let key_hash = hash_api_key(&raw_key);
        let key_id = format!("key_{}", Uuid::new_v4().simple());
        let record = ApiKeyRecord {
            key_id: key_id.clone(),
            tenant_id: req.tenant_id,
            name: req.name,
            key_prefix,
            qpm_limit: req.qpm_limit.max(1),
            tpm_limit: req.tpm_limit.max(1),
            namespace: if req.namespace.trim().is_empty() {
                "default".to_string()
            } else {
                req.namespace
            },
            revoked: false,
            created_at: Utc::now(),
            last_used_at: None,
        };
        self.id_to_hash.insert(key_id, key_hash.clone());
        self.by_hash.insert(key_hash, record.clone());
        Ok(IssuedApiKey { record, raw_key })
    }

    async fn seed_key(
        &self,
        raw_key: &str,
        req: IssueApiKeyRequest,
        revoked: bool,
    ) -> StorageResult<ApiKeyRecord> {
        if raw_key.trim().is_empty() {
            return Err(StorageError::Validation(
                "raw_key must not be empty".to_string(),
            ));
        }
        let key_hash = hash_api_key(raw_key);
        let key_prefix = raw_key.chars().take(16).collect::<String>();
        let namespace = if req.namespace.trim().is_empty() {
            "default".to_string()
        } else {
            req.namespace
        };
        let (key_id, created_at, last_used_at) = if let Some(existing) = self.by_hash.get(&key_hash)
        {
            (
                existing.key_id.clone(),
                existing.created_at,
                existing.last_used_at,
            )
        } else {
            (
                format!("key_{}", Uuid::new_v4().simple()),
                Utc::now(),
                None,
            )
        };
        let record = ApiKeyRecord {
            key_id: key_id.clone(),
            tenant_id: req.tenant_id,
            name: req.name,
            key_prefix,
            qpm_limit: req.qpm_limit.max(1),
            tpm_limit: req.tpm_limit.max(1),
            namespace,
            revoked,
            created_at,
            last_used_at,
        };
        self.id_to_hash.insert(key_id, key_hash.clone());
        self.by_hash.insert(key_hash, record.clone());
        Ok(record)
    }

    async fn verify_key(&self, raw_key: &str) -> StorageResult<Option<ApiKeyRecord>> {
        if raw_key.trim().is_empty() {
            return Ok(None);
        }
        let key_hash = hash_api_key(raw_key);
        let Some(mut entry) = self.by_hash.get_mut(&key_hash) else {
            return Ok(None);
        };
        if entry.revoked {
            return Ok(None);
        }
        entry.last_used_at = Some(Utc::now());
        Ok(Some(entry.clone()))
    }

    async fn revoke_key(&self, key_id: &str) -> StorageResult<bool> {
        let Some(hash) = self.id_to_hash.get(key_id).map(|h| h.clone()) else {
            return Ok(false);
        };
        if let Some(mut entry) = self.by_hash.get_mut(&hash)
            && !entry.revoked
        {
            entry.revoked = true;
            return Ok(true);
        }
        Ok(false)
    }

    async fn list_keys(&self) -> StorageResult<Vec<ApiKeyRecord>> {
        let mut keys: Vec<ApiKeyRecord> =
            self.by_hash.iter().map(|e| e.value().clone()).collect();
        keys.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        Ok(keys)
    }
}

#[derive(Default)]
pub struct InMemoryLlmProviderStore {
    providers: DashMap<String, ProviderConfig>,
    router: RwLock<RouterConfig>,
}

impl InMemoryLlmProviderStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl LlmProviderStore for InMemoryLlmProviderStore {
    async fn list_providers(&self) -> StorageResult<Vec<ProviderConfig>> {
        let mut list: Vec<ProviderConfig> =
            self.providers.iter().map(|e| e.value().clone()).collect();
        list.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.id.cmp(&b.id)));
        Ok(list)
    }

    async fn get_provider(&self, id: &str) -> StorageResult<Option<ProviderConfig>> {
        Ok(self.providers.get(id).map(|e| e.value().clone()))
    }

    async fn upsert_provider(&self, config: &ProviderConfig) -> StorageResult<()> {
        self.providers.insert(config.id.clone(), config.clone());
        Ok(())
    }

    async fn delete_provider(&self, id: &str) -> StorageResult<bool> {
        Ok(self.providers.remove(id).is_some())
    }

    async fn get_router_config(&self) -> StorageResult<RouterConfig> {
        Ok(self.router.read().clone())
    }

    async fn save_router_config(&self, config: &RouterConfig) -> StorageResult<()> {
        *self.router.write() = config.clone();
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryCheckpointStore {
    checkpoints: DashMap<String, IngestionCheckpoint>,
}

impl InMemoryCheckpointStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CheckpointStore for InMemoryCheckpointStore {
    async fn get_checkpoint(&self, job_id: &str) -> StorageResult<Option<IngestionCheckpoint>> {
        Ok(self.checkpoints.get(job_id).map(|e| e.value().clone()))
    }

    async fn save_checkpoint(&self, cp: &IngestionCheckpoint) -> StorageResult<()> {
        self.checkpoints.insert(cp.job_id.clone(), cp.clone());
        Ok(())
    }

    async fn list_checkpoints(&self, limit: usize) -> StorageResult<Vec<IngestionCheckpoint>> {
        let mut list: Vec<IngestionCheckpoint> =
            self.checkpoints.iter().map(|e| e.value().clone()).collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.updated_at));
        list.truncate(limit);
        Ok(list)
    }

    async fn reset_checkpoint(&self, job_id: &str) -> StorageResult<bool> {
        Ok(self.checkpoints.remove(job_id).is_some())
    }
}

#[derive(Default)]
pub struct InMemorySessionCache {
    sessions: DashMap<String, (SessionState, Instant)>,
    patient_sessions: DashMap<String, HashSet<String>>,
    semantic: DashMap<String, (String, Instant)>,
}

impl InMemorySessionCache {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl SessionCache for InMemorySessionCache {
    async fn get_session(&self, session_id: &str) -> StorageResult<Option<SessionState>> {
        let now = Instant::now();
        if let Some(entry) = self.sessions.get(session_id) {
            if now < entry.1 {
                return Ok(Some(entry.0.clone()));
            }
        } else {
            return Ok(None);
        }
        self.sessions.remove(session_id);
        Ok(None)
    }

    async fn save_session(&self, session: &SessionState, ttl_secs: u64) -> StorageResult<()> {
        let expires_at = Instant::now() + Duration::from_secs(ttl_secs.max(1));
        self.sessions
            .insert(session.session_id.clone(), (session.clone(), expires_at));
        self.patient_sessions
            .entry(session.patient_id.clone())
            .or_default()
            .insert(session.session_id.clone());
        Ok(())
    }

    async fn delete_session(&self, session_id: &str) -> StorageResult<bool> {
        Ok(self.sessions.remove(session_id).is_some())
    }

    async fn delete_patient_sessions(&self, patient_id: &str) -> StorageResult<u64> {
        let Some((_, ids)) = self.patient_sessions.remove(patient_id) else {
            return Ok(0);
        };
        let mut deleted = 0_u64;
        for sid in ids {
            if self.sessions.remove(&sid).is_some() {
                deleted += 1;
            }
        }
        Ok(deleted)
    }

    async fn get_cached_response(&self, cache_key: &str) -> StorageResult<Option<String>> {
        let now = Instant::now();
        if let Some(entry) = self.semantic.get(cache_key) {
            if now < entry.1 {
                return Ok(Some(entry.0.clone()));
            }
        } else {
            return Ok(None);
        }
        self.semantic.remove(cache_key);
        Ok(None)
    }

    async fn set_cached_response(
        &self,
        cache_key: &str,
        payload: &str,
        ttl_secs: u64,
    ) -> StorageResult<()> {
        let expires_at = Instant::now() + Duration::from_secs(ttl_secs.max(1));
        self.semantic
            .insert(cache_key.to_string(), (payload.to_string(), expires_at));
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryKnowledgeStore {
    chunks: DashMap<String, VectorizedKnowledgeChunk>,
}

impl InMemoryKnowledgeStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_documents(&self, docs: Vec<KnowledgeDocument>) {
        for doc in docs {
            let chunk = VectorizedKnowledgeChunk {
                document: doc,
                vector: Vec::new(),
                tenant_namespace: "default".to_string(),
            };
            self.chunks
                .insert(format!("default:{}", chunk.document.doc_id), chunk);
        }
    }

    fn matches_filter(chunk: &VectorizedKnowledgeChunk, filter: &KnowledgeVectorFilter) -> bool {
        if let Some(ns) = &filter.tenant_namespace {
            let ns_trimmed = ns.trim();
            let effective_ns = if ns_trimmed.is_empty() {
                "default"
            } else {
                ns_trimmed
            };
            if chunk.tenant_namespace != effective_ns
                && chunk.tenant_namespace != "default"
                && chunk.tenant_namespace != "public"
            {
                return false;
            }
        }
        if let Some(dept) = &filter.department
            && &chunk.document.department != dept
            && !chunk.document.secondary_departments.contains(dept)
        {
            return false;
        }
        if let Some(pops) = &filter.target_population
            && !pops.is_empty()
            && !pops
                .iter()
                .any(|p| chunk.document.target_population.contains(p))
        {
            return false;
        }
        true
    }

    fn keyword_score(doc: &KnowledgeDocument, query_text: &str, keywords: &[String]) -> f32 {
        let haystack = format!(
            "{} {} {} {}",
            doc.disease_name,
            doc.aliases.join(" "),
            doc.key_symptoms.join(" "),
            doc.content
        );
        let mut hits = 0.0_f32;
        for kw in keywords {
            if !kw.trim().is_empty() && haystack.contains(kw.trim()) {
                hits += 1.0;
            }
        }
        let q_chars: HashSet<char> = query_text.chars().filter(|c| !c.is_whitespace()).collect();
        let char_overlap = if q_chars.is_empty() {
            0.0
        } else {
            let matched = q_chars
                .iter()
                .filter(|&&c| haystack.contains(c))
                .count() as f32;
            matched / q_chars.len() as f32
        };
        let kw_norm = if keywords.is_empty() {
            0.0
        } else {
            (hits / keywords.len() as f32).min(1.0)
        };
        (0.6 * kw_norm + 0.4 * char_overlap).clamp(0.0, 1.0)
    }

    fn to_hit(
        doc: &KnowledgeDocument,
        tenant_namespace: Option<String>,
        vector_score: Option<f32>,
        bm25_score: Option<f32>,
        rrf_score: f32,
    ) -> KnowledgeSearchHit {
        KnowledgeSearchHit {
            doc_id: doc.doc_id.clone(),
            source_title: doc.source_title.clone(),
            source_type: doc.source_type,
            authority_level: doc.authority_level,
            disease_name: doc.disease_name.clone(),
            department: doc.department.clone(),
            secondary_departments: doc.secondary_departments.clone(),
            target_population: doc.target_population.clone(),
            section_type: doc.section_type,
            content: doc.content.clone(),
            key_symptoms: doc.key_symptoms.clone(),
            red_flag_signs: doc.red_flag_signs.clone(),
            seasonal_tags: doc.seasonal_tags.clone(),
            suggested_follow_up_slots: doc.suggested_follow_up_slots.clone(),
            vector_score,
            bm25_score,
            rrf_score,
            tenant_namespace,
        }
    }
}

#[async_trait]
impl KnowledgeVectorStore for InMemoryKnowledgeStore {
    async fn ensure_collection(&self, _vector_dim: u64) -> StorageResult<()> {
        Ok(())
    }

    async fn upsert_documents(&self, chunks: &[VectorizedKnowledgeChunk]) -> StorageResult<u64> {
        let mut count = 0_u64;
        for c in chunks {
            let key = format!("{}:{}", c.tenant_namespace, c.document.doc_id);
            self.chunks.insert(key, c.clone());
            count += 1;
        }
        Ok(count)
    }

    async fn search_vectors(
        &self,
        vector: &[f32],
        filter: &KnowledgeVectorFilter,
        top_k: usize,
    ) -> StorageResult<Vec<KnowledgeSearchHit>> {
        let mut hits: Vec<KnowledgeSearchHit> = self
            .chunks
            .iter()
            .filter(|e| Self::matches_filter(e.value(), filter))
            .map(|e| {
                let c = e.value();
                let vscore = if !c.vector.is_empty() && !vector.is_empty() {
                    cosine_similarity(vector, &c.vector).max(0.0)
                } else {
                    0.0
                };
                Self::to_hit(
                    &c.document,
                    Some(c.tenant_namespace.clone()),
                    Some(vscore),
                    None,
                    vscore,
                )
            })
            .collect();
        hits.sort_by(|a, b| {
            b.rrf_score
                .partial_cmp(&a.rrf_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(top_k.max(1));
        Ok(hits)
    }

    async fn delete_by_doc_ids(&self, doc_ids: &[String]) -> StorageResult<u64> {
        let set: HashSet<&str> = doc_ids.iter().map(String::as_str).collect();
        let keys: Vec<String> = self
            .chunks
            .iter()
            .filter(|e| set.contains(e.value().document.doc_id.as_str()))
            .map(|e| e.key().clone())
            .collect();
        let mut deleted = 0_u64;
        for k in keys {
            if self.chunks.remove(&k).is_some() {
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

#[async_trait]
impl KnowledgeSearchEngine for InMemoryKnowledgeStore {
    async fn search(&self, query: &KnowledgeSearchQuery) -> StorageResult<Vec<KnowledgeSearchHit>> {
        let filter = KnowledgeVectorFilter {
            target_population: query.target_population.clone(),
            department: query.department.clone(),
            tenant_namespace: query.tenant_namespace.clone(),
        };
        let mut hits: Vec<KnowledgeSearchHit> = self
            .chunks
            .iter()
            .filter(|e| Self::matches_filter(e.value(), &filter))
            .map(|e| {
                let c = e.value();
                let vscore = query.query_vector.as_ref().and_then(|qv| {
                    if !c.vector.is_empty() && !qv.is_empty() {
                        Some(cosine_similarity(qv, &c.vector).max(0.0))
                    } else {
                        None
                    }
                });
                let bm25 = Self::keyword_score(&c.document, &query.query_text, &query.keywords);
                let combined = match vscore {
                    Some(v) => 0.55 * v + 0.45 * bm25,
                    None => bm25,
                };
                Self::to_hit(
                    &c.document,
                    Some(c.tenant_namespace.clone()),
                    vscore,
                    Some(bm25),
                    combined,
                )
            })
            .collect();

        hits.sort_by(|a, b| {
            b.rrf_score
                .partial_cmp(&a.rrf_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(query.top_k.max(1));
        Ok(hits)
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom > 0.0 {
        dot / denom
    } else {
        0.0
    }
}
