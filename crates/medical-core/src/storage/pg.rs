use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use sqlx::types::Json;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::crypto::{FieldCipher, hash_api_key};
use crate::llm::{ModelTier, ProviderConfig, ProviderKind, RouterConfig};
use crate::models::{
    EpidemicContext, Gender, L2Profile, L2SensitivePayload, L3SensitivePayload, L3VisitEvent,
    MedicalDictionaries, PatientRecord, RedFlagLevel, RedFlagRule, Season, SymptomSynonym,
};

use super::traits::{
    ApiKeyRecord, ApiKeyStore, AuditLogStore, CheckpointStatus, CheckpointStore, DictionaryStore,
    EpidemicStore, GatewayMetricsSummary, IngestionCheckpoint, IssueApiKeyRequest, IssuedApiKey,
    LlmProviderStore, PatientL2Record, PatientMemoryStore, ProviderUsageStat, TriageAuditLog,
    WipedMemoryStats,
};
use super::{StorageError, StorageResult};

pub fn connect_pg_lazy(database_url: &str, max_connections: u32) -> StorageResult<PgPool> {
    PgPoolOptions::new()
        .max_connections(max_connections.max(1))
        .acquire_timeout(std::time::Duration::from_secs(5))
        .idle_timeout(Some(std::time::Duration::from_secs(300)))
        .max_lifetime(Some(std::time::Duration::from_secs(1800)))
        .connect_lazy(database_url)
        .map_err(StorageError::from)
}

#[derive(FromRow)]
struct L2Row {
    patient_id: String,
    consent_long_term_memory: bool,
    age_group: String,
    gender: String,
    is_pregnant: bool,
    encrypted_payload: Vec<u8>,
    encryption_nonce: Vec<u8>,
    key_version: i32,
    updated_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct L3Row {
    event_id: String,
    occurred_at: DateTime<Utc>,
    extracted_symptoms: Json<Vec<String>>,
    recommended_department: String,
    urgency_level: i16,
    encrypted_payload: Vec<u8>,
    encryption_nonce: Vec<u8>,
    key_version: i32,
}

#[derive(Clone)]
pub struct PgPatientMemoryStore {
    pool: PgPool,
    cipher: Arc<FieldCipher>,
    tenant_id: String,
}

impl PgPatientMemoryStore {
    pub fn new(pool: PgPool, cipher: Arc<FieldCipher>) -> Self {
        Self {
            pool,
            cipher,
            tenant_id: "default".to_string(),
        }
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }

    fn decode_l2_row(&self, row: L2Row) -> StorageResult<PatientL2Record> {
        let plaintext = self.cipher.decrypt_parts(
            row.key_version as u32,
            &row.encryption_nonce,
            &row.encrypted_payload,
        )?;
        let sensitive: L2SensitivePayload = serde_json::from_slice(&plaintext)?;
        Ok(PatientL2Record {
            patient_id: row.patient_id,
            consent_long_term_memory: row.consent_long_term_memory,
            l2_profile: L2Profile {
                age_group: row.age_group,
                gender: Gender::from_str_lossy(&row.gender),
                is_pregnant: row.is_pregnant,
                chronic_diseases: sensitive.chronic_diseases,
                allergies: sensitive.allergies,
                surgical_history: sensitive.surgical_history,
                long_term_medications: sensitive.long_term_medications,
                baseline_vitals: sensitive.baseline_vitals,
                trigger_substances: sensitive.trigger_substances,
            },
            updated_at: row.updated_at,
        })
    }

    fn decode_l3_row(&self, row: L3Row) -> StorageResult<L3VisitEvent> {
        let plaintext = self.cipher.decrypt_parts(
            row.key_version as u32,
            &row.encryption_nonce,
            &row.encrypted_payload,
        )?;
        let sensitive: L3SensitivePayload = serde_json::from_slice(&plaintext)?;
        Ok(L3VisitEvent {
            event_id: row.event_id,
            occurred_at: row.occurred_at,
            chief_complaint: sensitive.chief_complaint,
            extracted_symptoms: row.extracted_symptoms.0,
            recommended_department: row.recommended_department,
            urgency_level: row.urgency_level.clamp(1, 4) as u8,
            summary: sensitive.summary,
            clinical_exams: sensitive.clinical_exams,
        })
    }
}

#[async_trait]
impl PatientMemoryStore for PgPatientMemoryStore {
    async fn get_l2_profile(&self, patient_id: &str) -> StorageResult<Option<PatientL2Record>> {
        let row = sqlx::query_as::<_, L2Row>(
            r#"
            SELECT patient_id, consent_long_term_memory, age_group, gender, is_pregnant,
                   encrypted_payload, encryption_nonce, key_version, updated_at
            FROM patient_l2_profiles
            WHERE tenant_id = $1 AND patient_id = $2
            "#,
        )
        .bind(&self.tenant_id)
        .bind(patient_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(self.decode_l2_row(r)?)),
            None => Ok(None),
        }
    }

    async fn upsert_l2_profile(
        &self,
        patient_id: &str,
        consent: bool,
        profile: &L2Profile,
    ) -> StorageResult<()> {
        let sensitive = L2SensitivePayload::from(profile);
        let env = self.cipher.encrypt_json(&sensitive)?;

        sqlx::query(
            r#"
            INSERT INTO patient_l2_profiles (
                tenant_id, patient_id, consent_long_term_memory, age_group, gender, is_pregnant,
                encrypted_payload, encryption_nonce, key_version, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW(), NOW())
            ON CONFLICT (tenant_id, patient_id) DO UPDATE SET
                consent_long_term_memory = EXCLUDED.consent_long_term_memory,
                age_group = EXCLUDED.age_group,
                gender = EXCLUDED.gender,
                is_pregnant = EXCLUDED.is_pregnant,
                encrypted_payload = EXCLUDED.encrypted_payload,
                encryption_nonce = EXCLUDED.encryption_nonce,
                key_version = EXCLUDED.key_version,
                updated_at = NOW()
            "#,
        )
        .bind(&self.tenant_id)
        .bind(patient_id)
        .bind(consent)
        .bind(&profile.age_group)
        .bind(profile.gender.as_str())
        .bind(profile.is_pregnant)
        .bind(&env.ciphertext)
        .bind(env.nonce.as_slice())
        .bind(env.key_version as i32)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_l3_event(&self, patient_id: &str, event: &L3VisitEvent) -> StorageResult<()> {
        let sensitive = L3SensitivePayload::from(event);
        let env = self.cipher.encrypt_json(&sensitive)?;

        let mut tx = self.pool.begin().await?;

        // Ensure parent L2 profile row exists to satisfy FK constraint.
        let default_env = self.cipher.encrypt_json(&L2SensitivePayload::default())?;
        sqlx::query(
            r#"
            INSERT INTO patient_l2_profiles (
                tenant_id, patient_id, consent_long_term_memory, age_group, gender, is_pregnant,
                encrypted_payload, encryption_nonce, key_version, created_at, updated_at
            )
            VALUES ($1, $2, FALSE, 'unknown', 'unknown', FALSE, $3, $4, $5, NOW(), NOW())
            ON CONFLICT (tenant_id, patient_id) DO NOTHING
            "#,
        )
        .bind(&self.tenant_id)
        .bind(patient_id)
        .bind(&default_env.ciphertext)
        .bind(default_env.nonce.as_slice())
        .bind(default_env.key_version as i32)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO patient_l3_visit_events (
                event_id, tenant_id, patient_id, occurred_at, extracted_symptoms,
                recommended_department, urgency_level, encrypted_payload, encryption_nonce,
                key_version, qdrant_synced, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, FALSE, NOW())
            ON CONFLICT (tenant_id, event_id) DO UPDATE SET
                occurred_at = EXCLUDED.occurred_at,
                extracted_symptoms = EXCLUDED.extracted_symptoms,
                recommended_department = EXCLUDED.recommended_department,
                urgency_level = EXCLUDED.urgency_level,
                encrypted_payload = EXCLUDED.encrypted_payload,
                encryption_nonce = EXCLUDED.encryption_nonce,
                key_version = EXCLUDED.key_version
            "#,
        )
        .bind(&event.event_id)
        .bind(&self.tenant_id)
        .bind(patient_id)
        .bind(event.occurred_at)
        .bind(Json(&event.extracted_symptoms))
        .bind(&event.recommended_department)
        .bind(event.urgency_level as i16)
        .bind(&env.ciphertext)
        .bind(env.nonce.as_slice())
        .bind(env.key_version as i32)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn list_l3_events(&self, patient_id: &str, limit: usize) -> StorageResult<Vec<L3VisitEvent>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, L3Row>(
            r#"
            SELECT event_id, occurred_at, extracted_symptoms, recommended_department,
                   urgency_level, encrypted_payload, encryption_nonce, key_version
            FROM patient_l3_visit_events
            WHERE tenant_id = $1 AND patient_id = $2
            ORDER BY occurred_at DESC
            LIMIT $3
            "#,
        )
        .bind(&self.tenant_id)
        .bind(patient_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| self.decode_l3_row(r)).collect()
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
        let rows: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT patient_id
            FROM patient_l2_profiles
            WHERE tenant_id = $1
            ORDER BY updated_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(&self.tenant_id)
        .bind(limit.max(1) as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| r.0).collect())
    }

    async fn delete_patient_memory(&self, patient_id: &str) -> StorageResult<WipedMemoryStats> {
        let mut tx = self.pool.begin().await?;
        let l3_res = sqlx::query(
            "DELETE FROM patient_l3_visit_events WHERE tenant_id = $1 AND patient_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(patient_id)
        .execute(&mut *tx)
        .await?;

        let l2_res = sqlx::query(
            "DELETE FROM patient_l2_profiles WHERE tenant_id = $1 AND patient_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(patient_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(WipedMemoryStats {
            l2_profiles_deleted: l2_res.rows_affected(),
            l3_events_deleted: l3_res.rows_affected(),
            qdrant_points_deleted: 0,
            l1_sessions_deleted: 0,
        })
    }
}

#[derive(FromRow)]
struct EpidemicRow {
    context_id: String,
    region_code: String,
    region_name: String,
    season: String,
    disease_name: String,
    seasonal_tag: String,
    intensity: f32,
    boost_symptoms: Json<Vec<String>>,
    priority_question_hint: String,
    active: bool,
    valid_from: NaiveDate,
    valid_to: NaiveDate,
}

impl EpidemicRow {
    fn into_model(self) -> StorageResult<EpidemicContext> {
        let season = Season::from_str_checked(&self.season).ok_or_else(|| {
            StorageError::Serialization(format!("unknown season in DB: {}", self.season))
        })?;
        Ok(EpidemicContext {
            context_id: self.context_id,
            region_code: self.region_code,
            region_name: self.region_name,
            season,
            disease_name: self.disease_name,
            seasonal_tag: self.seasonal_tag,
            intensity: self.intensity,
            boost_symptoms: self.boost_symptoms.0,
            priority_question_hint: self.priority_question_hint,
            active: self.active,
            valid_from: self.valid_from,
            valid_to: self.valid_to,
        })
    }
}

#[derive(Clone)]
pub struct PgEpidemicStore {
    pool: PgPool,
    tenant_id: String,
}

impl PgEpidemicStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            tenant_id: "default".to_string(),
        }
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }
}

#[async_trait]
impl EpidemicStore for PgEpidemicStore {
    async fn list_active(
        &self,
        region_code: Option<&str>,
        season: Option<&str>,
        as_of: NaiveDate,
    ) -> StorageResult<Vec<EpidemicContext>> {
        let rows = sqlx::query_as::<_, EpidemicRow>(
            r#"
            SELECT context_id, region_code, region_name, season, disease_name, seasonal_tag,
                   intensity, boost_symptoms, priority_question_hint, active, valid_from, valid_to
            FROM epidemic_contexts
            WHERE tenant_id = $1
              AND active = TRUE
              AND valid_from <= $2
              AND valid_to >= $2
              AND ($3::text IS NULL OR region_code = $3)
              AND ($4::text IS NULL OR season = $4 OR season = 'all_year')
            ORDER BY intensity DESC
            "#,
        )
        .bind(&self.tenant_id)
        .bind(as_of)
        .bind(region_code)
        .bind(season)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(EpidemicRow::into_model).collect()
    }

    async fn list_all(&self) -> StorageResult<Vec<EpidemicContext>> {
        let rows = sqlx::query_as::<_, EpidemicRow>(
            r#"
            SELECT context_id, region_code, region_name, season, disease_name, seasonal_tag,
                   intensity, boost_symptoms, priority_question_hint, active, valid_from, valid_to
            FROM epidemic_contexts
            WHERE tenant_id = $1
            ORDER BY updated_at DESC
            "#,
        )
        .bind(&self.tenant_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(EpidemicRow::into_model).collect()
    }

    async fn upsert(&self, ctx: &EpidemicContext) -> StorageResult<()> {
        sqlx::query(
            r#"
            INSERT INTO epidemic_contexts (
                context_id, tenant_id, region_code, region_name, season, disease_name,
                seasonal_tag, intensity, boost_symptoms, priority_question_hint, active,
                valid_from, valid_to, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, NOW(), NOW())
            ON CONFLICT (tenant_id, context_id) DO UPDATE SET
                region_code = EXCLUDED.region_code,
                region_name = EXCLUDED.region_name,
                season = EXCLUDED.season,
                disease_name = EXCLUDED.disease_name,
                seasonal_tag = EXCLUDED.seasonal_tag,
                intensity = EXCLUDED.intensity,
                boost_symptoms = EXCLUDED.boost_symptoms,
                priority_question_hint = EXCLUDED.priority_question_hint,
                active = EXCLUDED.active,
                valid_from = EXCLUDED.valid_from,
                valid_to = EXCLUDED.valid_to,
                updated_at = NOW()
            "#,
        )
        .bind(&ctx.context_id)
        .bind(&self.tenant_id)
        .bind(&ctx.region_code)
        .bind(&ctx.region_name)
        .bind(ctx.season.as_str())
        .bind(&ctx.disease_name)
        .bind(&ctx.seasonal_tag)
        .bind(ctx.intensity)
        .bind(Json(&ctx.boost_symptoms))
        .bind(&ctx.priority_question_hint)
        .bind(ctx.active)
        .bind(ctx.valid_from)
        .bind(ctx.valid_to)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, context_id: &str) -> StorageResult<bool> {
        let res = sqlx::query(
            "DELETE FROM epidemic_contexts WHERE tenant_id = $1 AND context_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(context_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

#[derive(FromRow)]
struct SynonymRow {
    standard_term: String,
    colloquial_terms: Json<Vec<String>>,
}

#[derive(FromRow)]
struct RedFlagRow {
    rule_id: String,
    level: String,
    trigger_all_of: Json<Vec<String>>,
    trigger_any_of: Json<Vec<String>>,
    alert_message: String,
}

#[derive(Clone)]
pub struct PgDictionaryStore {
    pool: PgPool,
    tenant_id: String,
}

impl PgDictionaryStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            tenant_id: "default".to_string(),
        }
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }
}

#[async_trait]
impl DictionaryStore for PgDictionaryStore {
    async fn load_dictionaries(&self) -> StorageResult<MedicalDictionaries> {
        let (synonyms, rules) = tokio::try_join!(
            self.list_symptom_synonyms(),
            self.list_red_flag_rules()
        )?;
        Ok(MedicalDictionaries {
            symptom_synonyms: synonyms,
            red_flag_rules: rules,
        })
    }

    async fn list_symptom_synonyms(&self) -> StorageResult<Vec<SymptomSynonym>> {
        let rows = sqlx::query_as::<_, SynonymRow>(
            r#"
            SELECT standard_term, colloquial_terms
            FROM symptom_synonyms
            WHERE tenant_id = $1 AND enabled = TRUE
            ORDER BY standard_term ASC
            "#,
        )
        .bind(&self.tenant_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| SymptomSynonym {
                standard_term: r.standard_term,
                colloquial_terms: r.colloquial_terms.0,
            })
            .collect())
    }

    async fn save_symptom_synonym(&self, synonym: &SymptomSynonym) -> StorageResult<()> {
        sqlx::query(
            r#"
            INSERT INTO symptom_synonyms (
                tenant_id, standard_term, colloquial_terms, enabled, created_at, updated_at
            )
            VALUES ($1, $2, $3, TRUE, NOW(), NOW())
            ON CONFLICT (tenant_id, standard_term) DO UPDATE SET
                colloquial_terms = EXCLUDED.colloquial_terms,
                enabled = TRUE,
                updated_at = NOW()
            "#,
        )
        .bind(&self.tenant_id)
        .bind(&synonym.standard_term)
        .bind(Json(&synonym.colloquial_terms))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_symptom_synonym(&self, standard_term: &str) -> StorageResult<bool> {
        let res = sqlx::query(
            "DELETE FROM symptom_synonyms WHERE tenant_id = $1 AND standard_term = $2",
        )
        .bind(&self.tenant_id)
        .bind(standard_term)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn list_red_flag_rules(&self) -> StorageResult<Vec<RedFlagRule>> {
        let rows = sqlx::query_as::<_, RedFlagRow>(
            r#"
            SELECT rule_id, level, trigger_all_of, trigger_any_of, alert_message
            FROM red_flag_rules
            WHERE tenant_id = $1 AND enabled = TRUE
            ORDER BY rule_id ASC
            "#,
        )
        .bind(&self.tenant_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                let level = RedFlagLevel::from_str_checked(&r.level).ok_or_else(|| {
                    StorageError::Serialization(format!("unknown red_flag level: {}", r.level))
                })?;
                Ok(RedFlagRule {
                    rule_id: r.rule_id,
                    level,
                    trigger_all_of: r.trigger_all_of.0,
                    trigger_any_of: r.trigger_any_of.0,
                    alert_message: r.alert_message,
                })
            })
            .collect()
    }

    async fn save_red_flag_rule(&self, rule: &RedFlagRule) -> StorageResult<()> {
        sqlx::query(
            r#"
            INSERT INTO red_flag_rules (
                rule_id, tenant_id, level, trigger_all_of, trigger_any_of, alert_message,
                enabled, created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, TRUE, NOW(), NOW())
            ON CONFLICT (tenant_id, rule_id) DO UPDATE SET
                level = EXCLUDED.level,
                trigger_all_of = EXCLUDED.trigger_all_of,
                trigger_any_of = EXCLUDED.trigger_any_of,
                alert_message = EXCLUDED.alert_message,
                enabled = TRUE,
                updated_at = NOW()
            "#,
        )
        .bind(&rule.rule_id)
        .bind(&self.tenant_id)
        .bind(rule.level.as_str())
        .bind(Json(&rule.trigger_all_of))
        .bind(Json(&rule.trigger_any_of))
        .bind(&rule.alert_message)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_red_flag_rule(&self, rule_id: &str) -> StorageResult<bool> {
        let res = sqlx::query(
            "DELETE FROM red_flag_rules WHERE tenant_id = $1 AND rule_id = $2",
        )
        .bind(&self.tenant_id)
        .bind(rule_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}

#[derive(FromRow)]
struct AuditRow {
    id: Uuid,
    tenant_id: String,
    session_id: String,
    patient_id: String,
    red_flag: String,
    triage_status: String,
    recommended_department: Option<String>,
    urgency_level: Option<i16>,
    provider_id: Option<String>,
    model_name: Option<String>,
    prompt_tokens: i32,
    completion_tokens: i32,
    latency_ms: i64,
    seasonal_contexts_applied: Json<Vec<String>>,
    citation_doc_ids: Json<Vec<String>>,
    error_message: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct PgAuditLogStore {
    pool: PgPool,
    default_tenant_id: String,
}

impl PgAuditLogStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            default_tenant_id: "default".to_string(),
        }
    }
}

#[async_trait]
impl AuditLogStore for PgAuditLogStore {
    async fn append(&self, log: &TriageAuditLog) -> StorageResult<()> {
        let tenant = log
            .tenant_id
            .as_deref()
            .unwrap_or(&self.default_tenant_id);
        sqlx::query(
            r#"
            INSERT INTO triage_audit_logs (
                id, tenant_id, session_id, patient_id, red_flag, triage_status,
                recommended_department, urgency_level, provider_id, model_name,
                prompt_tokens, completion_tokens, latency_ms,
                seasonal_contexts_applied, citation_doc_ids, error_message, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
            "#,
        )
        .bind(log.id)
        .bind(tenant)
        .bind(&log.session_id)
        .bind(&log.patient_id)
        .bind(log.red_flag.as_str())
        .bind(&log.triage_status)
        .bind(&log.recommended_department)
        .bind(log.urgency_level.map(|u| u as i16))
        .bind(&log.provider_id)
        .bind(&log.model_name)
        .bind(log.prompt_tokens as i32)
        .bind(log.completion_tokens as i32)
        .bind(log.latency_ms as i64)
        .bind(Json(&log.seasonal_contexts_applied))
        .bind(Json(&log.citation_doc_ids))
        .bind(&log.error_message)
        .bind(log.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn query_recent(
        &self,
        limit: usize,
        tenant_id: Option<&str>,
    ) -> StorageResult<Vec<TriageAuditLog>> {
        let rows = sqlx::query_as::<_, AuditRow>(
            r#"
            SELECT id, tenant_id, session_id, patient_id, red_flag, triage_status,
                   recommended_department, urgency_level, provider_id, model_name,
                   prompt_tokens, completion_tokens, latency_ms,
                   seasonal_contexts_applied, citation_doc_ids, error_message, created_at
            FROM triage_audit_logs
            WHERE ($1::text IS NULL OR tenant_id = $1)
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(tenant_id)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| TriageAuditLog {
                id: r.id,
                session_id: r.session_id,
                patient_id: r.patient_id,
                tenant_id: Some(r.tenant_id),
                red_flag: RedFlagLevel::from_str_checked(&r.red_flag).unwrap_or_default(),
                triage_status: r.triage_status,
                recommended_department: r.recommended_department,
                urgency_level: r.urgency_level.map(|v| v.clamp(1, 4) as u8),
                provider_id: r.provider_id,
                model_name: r.model_name,
                prompt_tokens: r.prompt_tokens.max(0) as u32,
                completion_tokens: r.completion_tokens.max(0) as u32,
                latency_ms: r.latency_ms.max(0) as u64,
                seasonal_contexts_applied: r.seasonal_contexts_applied.0,
                citation_doc_ids: r.citation_doc_ids.0,
                error_message: r.error_message,
                created_at: r.created_at,
            })
            .collect())
    }

    async fn get_metrics_summary(&self, window_hours: u32) -> StorageResult<GatewayMetricsSummary> {
        let cutoff = Utc::now() - chrono::Duration::hours(window_hours.max(1) as i64);
        let rows = sqlx::query_as::<_, AuditRow>(
            r#"
            SELECT id, tenant_id, session_id, patient_id, red_flag, triage_status,
                   recommended_department, urgency_level, provider_id, model_name,
                   prompt_tokens, completion_tokens, latency_ms,
                   seasonal_contexts_applied, citation_doc_ids, error_message, created_at
            FROM triage_audit_logs
            WHERE created_at >= $1
            ORDER BY created_at DESC
            LIMIT 10000
            "#,
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        Ok(summarize_audit_logs(&rows))
    }
}

fn summarize_audit_logs(rows: &[AuditRow]) -> GatewayMetricsSummary {
    let mut total_requests = 0_u64;
    let mut total_errors = 0_u64;
    let mut total_prompt = 0_u64;
    let mut total_completion = 0_u64;
    let mut total_latency = 0_u64;
    let mut by_provider: HashMap<String, (u64, u64, u64, u64, u64)> = HashMap::new();

    for r in rows {
        total_requests += 1;
        let is_err = r.error_message.is_some();
        if is_err {
            total_errors += 1;
        }
        let p = r.prompt_tokens.max(0) as u64;
        let c = r.completion_tokens.max(0) as u64;
        let lat = r.latency_ms.max(0) as u64;
        total_prompt += p;
        total_completion += c;
        total_latency += lat;

        let pid = r.provider_id.clone().unwrap_or_else(|| "local_rule".to_string());
        let entry = by_provider.entry(pid).or_insert((0, 0, 0, 0, 0));
        entry.0 += 1;
        if is_err {
            entry.1 += 1;
        }
        entry.2 += p;
        entry.3 += c;
        entry.4 += lat;
    }

    let by_provider_vec = by_provider
        .into_iter()
        .map(|(provider_id, (req, err, pt, ct, lat))| ProviderUsageStat {
            provider_id,
            requests: req,
            errors: err,
            prompt_tokens: pt,
            completion_tokens: ct,
            avg_latency_ms: if req > 0 { lat as f64 / req as f64 } else { 0.0 },
        })
        .collect();

    GatewayMetricsSummary {
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
        by_provider: by_provider_vec,
    }
}

#[derive(FromRow)]
struct ApiKeyRow {
    key_id: String,
    tenant_id: String,
    name: String,
    key_prefix: String,
    qpm_limit: i32,
    tpm_limit: i32,
    namespace: String,
    revoked: bool,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

impl From<ApiKeyRow> for ApiKeyRecord {
    fn from(r: ApiKeyRow) -> Self {
        Self {
            key_id: r.key_id,
            tenant_id: r.tenant_id,
            name: r.name,
            key_prefix: r.key_prefix,
            qpm_limit: r.qpm_limit.max(1) as u32,
            tpm_limit: r.tpm_limit.max(1) as u32,
            namespace: r.namespace,
            revoked: r.revoked,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
        }
    }
}

#[derive(Clone)]
pub struct PgApiKeyStore {
    pool: PgPool,
}

impl PgApiKeyStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ApiKeyStore for PgApiKeyStore {
    async fn issue_key(&self, req: IssueApiKeyRequest) -> StorageResult<IssuedApiKey> {
        let mut raw_secret = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut raw_secret);
        let raw_key = format!("mk_live_{}", hex::encode(raw_secret));
        let key_prefix = raw_key.chars().take(16).collect::<String>();
        let key_hash = hash_api_key(&raw_key);
        let key_id = format!("key_{}", Uuid::new_v4().simple());
        let now = Utc::now();
        let namespace = if req.namespace.trim().is_empty() {
            "default".to_string()
        } else {
            req.namespace
        };

        sqlx::query(
            r#"
            INSERT INTO api_keys (
                key_id, tenant_id, name, key_prefix, key_hash, qpm_limit, tpm_limit,
                namespace, revoked, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, FALSE, $9)
            "#,
        )
        .bind(&key_id)
        .bind(&req.tenant_id)
        .bind(&req.name)
        .bind(&key_prefix)
        .bind(&key_hash)
        .bind(req.qpm_limit.max(1) as i32)
        .bind(req.tpm_limit.max(1) as i32)
        .bind(&namespace)
        .bind(now)
        .execute(&self.pool)
        .await?;

        Ok(IssuedApiKey {
            record: ApiKeyRecord {
                key_id,
                tenant_id: req.tenant_id,
                name: req.name,
                key_prefix,
                qpm_limit: req.qpm_limit.max(1),
                tpm_limit: req.tpm_limit.max(1),
                namespace,
                revoked: false,
                created_at: now,
                last_used_at: None,
            },
            raw_key,
        })
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
        let key_prefix = raw_key.chars().take(16).collect::<String>();
        let key_hash = hash_api_key(raw_key);
        let key_id = format!("key_{}", Uuid::new_v4().simple());
        let now = Utc::now();
        let namespace = if req.namespace.trim().is_empty() {
            "default".to_string()
        } else {
            req.namespace
        };
        let revoked_at = if revoked { Some(now) } else { None };

        let row = sqlx::query_as::<_, ApiKeyRow>(
            r#"
            INSERT INTO api_keys (
                key_id, tenant_id, name, key_prefix, key_hash, qpm_limit, tpm_limit,
                namespace, revoked, created_at, revoked_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (key_hash) DO UPDATE SET
                tenant_id = EXCLUDED.tenant_id,
                name = EXCLUDED.name,
                key_prefix = EXCLUDED.key_prefix,
                qpm_limit = EXCLUDED.qpm_limit,
                tpm_limit = EXCLUDED.tpm_limit,
                namespace = EXCLUDED.namespace,
                revoked = EXCLUDED.revoked,
                revoked_at = CASE
                    WHEN EXCLUDED.revoked THEN COALESCE(api_keys.revoked_at, NOW())
                    ELSE NULL
                END
            RETURNING key_id, tenant_id, name, key_prefix, qpm_limit, tpm_limit,
                      namespace, revoked, created_at, last_used_at
            "#,
        )
        .bind(&key_id)
        .bind(&req.tenant_id)
        .bind(&req.name)
        .bind(&key_prefix)
        .bind(&key_hash)
        .bind(req.qpm_limit.max(1) as i32)
        .bind(req.tpm_limit.max(1) as i32)
        .bind(&namespace)
        .bind(revoked)
        .bind(now)
        .bind(revoked_at)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.into())
    }

    async fn verify_key(&self, raw_key: &str) -> StorageResult<Option<ApiKeyRecord>> {
        if raw_key.trim().is_empty() {
            return Ok(None);
        }
        let key_hash = hash_api_key(raw_key);
        let row = sqlx::query_as::<_, ApiKeyRow>(
            r#"
            SELECT key_id, tenant_id, name, key_prefix, qpm_limit, tpm_limit,
                   namespace, revoked, created_at, last_used_at
            FROM api_keys
            WHERE key_hash = $1 AND revoked = FALSE
            "#,
        )
        .bind(&key_hash)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(r) = row {
            let _ = sqlx::query("UPDATE api_keys SET last_used_at = NOW() WHERE key_id = $1")
                .bind(&r.key_id)
                .execute(&self.pool)
                .await;
            Ok(Some(r.into()))
        } else {
            Ok(None)
        }
    }

    async fn revoke_key(&self, key_id: &str) -> StorageResult<bool> {
        let res = sqlx::query(
            "UPDATE api_keys SET revoked = TRUE, revoked_at = NOW() WHERE key_id = $1 AND revoked = FALSE",
        )
        .bind(key_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn list_keys(&self) -> StorageResult<Vec<ApiKeyRecord>> {
        let rows = sqlx::query_as::<_, ApiKeyRow>(
            r#"
            SELECT key_id, tenant_id, name, key_prefix, qpm_limit, tpm_limit,
                   namespace, revoked, created_at, last_used_at
            FROM api_keys
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}

#[derive(FromRow)]
struct ProviderRow {
    id: String,
    name: String,
    kind: String,
    base_url: String,
    encrypted_api_key: Vec<u8>,
    encryption_nonce: Vec<u8>,
    key_version: i32,
    model: String,
    tier: String,
    timeout_ms: i64,
    is_intranet: bool,
    enabled: bool,
    priority: i32,
}

#[derive(Clone)]
pub struct PgLlmProviderStore {
    pool: PgPool,
    cipher: Arc<FieldCipher>,
    tenant_id: String,
}

impl PgLlmProviderStore {
    pub fn new(pool: PgPool, cipher: Arc<FieldCipher>) -> Self {
        Self {
            pool,
            cipher,
            tenant_id: "default".to_string(),
        }
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }

    fn decode_row(&self, row: ProviderRow) -> StorageResult<ProviderConfig> {
        let key_bytes = self.cipher.decrypt_parts(
            row.key_version as u32,
            &row.encryption_nonce,
            &row.encrypted_api_key,
        )?;
        let api_key = String::from_utf8(key_bytes)
            .map_err(|e| StorageError::Crypto(format!("invalid UTF-8 api_key: {e}")))?;
        let kind = ProviderKind::from_str_checked(&row.kind).ok_or_else(|| {
            StorageError::Serialization(format!("unknown provider kind: {}", row.kind))
        })?;
        let tier = ModelTier::from_str_checked(&row.tier).ok_or_else(|| {
            StorageError::Serialization(format!("unknown model tier: {}", row.tier))
        })?;
        Ok(ProviderConfig {
            id: row.id,
            name: row.name,
            kind,
            base_url: row.base_url,
            api_key,
            model: row.model,
            tier,
            timeout_ms: row.timeout_ms.max(100) as u64,
            is_intranet: row.is_intranet,
            enabled: row.enabled,
            priority: row.priority,
        })
    }
}

#[async_trait]
impl LlmProviderStore for PgLlmProviderStore {
    async fn list_providers(&self) -> StorageResult<Vec<ProviderConfig>> {
        let rows = sqlx::query_as::<_, ProviderRow>(
            r#"
            SELECT id, name, kind, base_url, encrypted_api_key, encryption_nonce,
                   key_version, model, tier, timeout_ms, is_intranet, enabled, priority
            FROM llm_providers
            WHERE tenant_id = $1
            ORDER BY priority ASC, id ASC
            "#,
        )
        .bind(&self.tenant_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|r| self.decode_row(r)).collect()
    }

    async fn get_provider(&self, id: &str) -> StorageResult<Option<ProviderConfig>> {
        let row = sqlx::query_as::<_, ProviderRow>(
            r#"
            SELECT id, name, kind, base_url, encrypted_api_key, encryption_nonce,
                   key_version, model, tier, timeout_ms, is_intranet, enabled, priority
            FROM llm_providers
            WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(&self.tenant_id)
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(Some(self.decode_row(r)?)),
            None => Ok(None),
        }
    }

    async fn upsert_provider(&self, config: &ProviderConfig) -> StorageResult<()> {
        let env = self.cipher.encrypt_bytes(config.api_key.as_bytes())?;
        sqlx::query(
            r#"
            INSERT INTO llm_providers (
                id, tenant_id, name, kind, base_url, encrypted_api_key, encryption_nonce,
                key_version, model, tier, timeout_ms, is_intranet, enabled, priority,
                created_at, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, NOW(), NOW())
            ON CONFLICT (tenant_id, id) DO UPDATE SET
                name = EXCLUDED.name,
                kind = EXCLUDED.kind,
                base_url = EXCLUDED.base_url,
                encrypted_api_key = EXCLUDED.encrypted_api_key,
                encryption_nonce = EXCLUDED.encryption_nonce,
                key_version = EXCLUDED.key_version,
                model = EXCLUDED.model,
                tier = EXCLUDED.tier,
                timeout_ms = EXCLUDED.timeout_ms,
                is_intranet = EXCLUDED.is_intranet,
                enabled = EXCLUDED.enabled,
                priority = EXCLUDED.priority,
                updated_at = NOW()
            "#,
        )
        .bind(&config.id)
        .bind(&self.tenant_id)
        .bind(&config.name)
        .bind(config.kind.as_str())
        .bind(&config.base_url)
        .bind(&env.ciphertext)
        .bind(env.nonce.as_slice())
        .bind(env.key_version as i32)
        .bind(&config.model)
        .bind(config.tier.as_str())
        .bind(config.timeout_ms.max(100) as i64)
        .bind(config.is_intranet)
        .bind(config.enabled)
        .bind(config.priority)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_provider(&self, id: &str) -> StorageResult<bool> {
        let res = sqlx::query("DELETE FROM llm_providers WHERE tenant_id = $1 AND id = $2")
            .bind(&self.tenant_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    async fn get_router_config(&self) -> StorageResult<RouterConfig> {
        let row: Option<(Json<RouterConfig>,)> = sqlx::query_as(
            "SELECT config_json FROM llm_router_config WHERE tenant_id = $1",
        )
        .bind(&self.tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0.0).unwrap_or_default())
    }

    async fn save_router_config(&self, config: &RouterConfig) -> StorageResult<()> {
        sqlx::query(
            r#"
            INSERT INTO llm_router_config (tenant_id, config_json, updated_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (tenant_id) DO UPDATE SET
                config_json = EXCLUDED.config_json,
                updated_at = NOW()
            "#,
        )
        .bind(&self.tenant_id)
        .bind(Json(config))
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[derive(FromRow)]
struct CheckpointRow {
    job_id: String,
    source_path: String,
    job_type: String,
    status: String,
    total_items: i64,
    processed_items: i64,
    failed_items: i64,
    byte_offset: i64,
    last_doc_id: Option<String>,
    error_samples: Json<Vec<String>>,
    updated_at: DateTime<Utc>,
}

impl From<CheckpointRow> for IngestionCheckpoint {
    fn from(r: CheckpointRow) -> Self {
        Self {
            job_id: r.job_id,
            source_path: r.source_path,
            job_type: r.job_type,
            status: CheckpointStatus::from_str_lossy(&r.status),
            total_items: r.total_items.max(0) as u64,
            processed_items: r.processed_items.max(0) as u64,
            failed_items: r.failed_items.max(0) as u64,
            byte_offset: r.byte_offset.max(0) as u64,
            last_doc_id: r.last_doc_id,
            error_samples: r.error_samples.0,
            updated_at: r.updated_at,
        }
    }
}

#[derive(Clone)]
pub struct PgCheckpointStore {
    pool: PgPool,
    tenant_id: String,
}

impl PgCheckpointStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            tenant_id: "default".to_string(),
        }
    }

    pub fn with_tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }
}

#[async_trait]
impl CheckpointStore for PgCheckpointStore {
    async fn get_checkpoint(&self, job_id: &str) -> StorageResult<Option<IngestionCheckpoint>> {
        let row = sqlx::query_as::<_, CheckpointRow>(
            r#"
            SELECT job_id, source_path, job_type, status, total_items, processed_items,
                   failed_items, byte_offset, last_doc_id, error_samples, updated_at
            FROM ingestion_checkpoints
            WHERE job_id = $1 AND tenant_id = $2
            "#,
        )
        .bind(job_id)
        .bind(&self.tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(Into::into))
    }

    async fn save_checkpoint(&self, cp: &IngestionCheckpoint) -> StorageResult<()> {
        let completed_at = if cp.status == CheckpointStatus::Completed {
            Some(Utc::now())
        } else {
            None
        };
        sqlx::query(
            r#"
            INSERT INTO ingestion_checkpoints (
                job_id, tenant_id, source_path, job_type, status, total_items,
                processed_items, failed_items, byte_offset, last_doc_id, error_samples,
                started_at, updated_at, completed_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, NOW(), $12, $13)
            ON CONFLICT (job_id) DO UPDATE SET
                status = EXCLUDED.status,
                total_items = EXCLUDED.total_items,
                processed_items = EXCLUDED.processed_items,
                failed_items = EXCLUDED.failed_items,
                byte_offset = EXCLUDED.byte_offset,
                last_doc_id = EXCLUDED.last_doc_id,
                error_samples = EXCLUDED.error_samples,
                updated_at = EXCLUDED.updated_at,
                completed_at = COALESCE(EXCLUDED.completed_at, ingestion_checkpoints.completed_at)
            "#,
        )
        .bind(&cp.job_id)
        .bind(&self.tenant_id)
        .bind(&cp.source_path)
        .bind(&cp.job_type)
        .bind(cp.status.as_str())
        .bind(cp.total_items as i64)
        .bind(cp.processed_items as i64)
        .bind(cp.failed_items as i64)
        .bind(cp.byte_offset as i64)
        .bind(&cp.last_doc_id)
        .bind(Json(&cp.error_samples))
        .bind(cp.updated_at)
        .bind(completed_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_checkpoints(&self, limit: usize) -> StorageResult<Vec<IngestionCheckpoint>> {
        let rows = sqlx::query_as::<_, CheckpointRow>(
            r#"
            SELECT job_id, source_path, job_type, status, total_items, processed_items,
                   failed_items, byte_offset, last_doc_id, error_samples, updated_at
            FROM ingestion_checkpoints
            WHERE tenant_id = $1
            ORDER BY updated_at DESC
            LIMIT $2
            "#,
        )
        .bind(&self.tenant_id)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn reset_checkpoint(&self, job_id: &str) -> StorageResult<bool> {
        let res = sqlx::query(
            "DELETE FROM ingestion_checkpoints WHERE job_id = $1 AND tenant_id = $2",
        )
        .bind(job_id)
        .bind(&self.tenant_id)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }
}
