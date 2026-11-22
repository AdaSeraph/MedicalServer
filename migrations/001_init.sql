-- MedicalServer PostgreSQL Initial Schema (PostgreSQL 14+)
-- Compliance: Sensitive L2/L3 health fields & upstream API keys encrypted with AES-256-GCM.
-- Every table and column matches crates/medical-core/src/storage/pg.rs 1:1.

CREATE TABLE IF NOT EXISTS patient_l2_profiles (
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    patient_id               VARCHAR(128) NOT NULL,
    consent_long_term_memory BOOLEAN NOT NULL DEFAULT FALSE,
    age_group                VARCHAR(32) NOT NULL DEFAULT 'unknown',
    gender                   VARCHAR(32) NOT NULL DEFAULT 'unknown',
    is_pregnant              BOOLEAN NOT NULL DEFAULT FALSE,
    encrypted_payload        BYTEA NOT NULL,
    encryption_nonce         BYTEA NOT NULL CHECK (octet_length(encryption_nonce) = 12),
    key_version              INTEGER NOT NULL DEFAULT 1,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, patient_id)
);

CREATE INDEX IF NOT EXISTS idx_l2_profiles_updated_at
    ON patient_l2_profiles (tenant_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS patient_l3_visit_events (
    event_id                 VARCHAR(128) NOT NULL,
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    patient_id               VARCHAR(128) NOT NULL,
    occurred_at              TIMESTAMPTZ NOT NULL,
    extracted_symptoms       JSONB NOT NULL DEFAULT '[]'::jsonb,
    recommended_department   VARCHAR(128) NOT NULL,
    urgency_level            SMALLINT NOT NULL CHECK (urgency_level BETWEEN 1 AND 4),
    encrypted_payload        BYTEA NOT NULL,
    encryption_nonce         BYTEA NOT NULL CHECK (octet_length(encryption_nonce) = 12),
    key_version              INTEGER NOT NULL DEFAULT 1,
    qdrant_synced            BOOLEAN NOT NULL DEFAULT FALSE,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, event_id),
    FOREIGN KEY (tenant_id, patient_id)
        REFERENCES patient_l2_profiles (tenant_id, patient_id)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_l3_events_patient_occurred
    ON patient_l3_visit_events (tenant_id, patient_id, occurred_at DESC);

CREATE TABLE IF NOT EXISTS epidemic_contexts (
    context_id               VARCHAR(128) NOT NULL,
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    region_code              VARCHAR(32) NOT NULL,
    region_name              VARCHAR(128) NOT NULL,
    season                   VARCHAR(32) NOT NULL CHECK (season IN ('spring', 'summer', 'autumn', 'winter', 'all_year')),
    disease_name             VARCHAR(128) NOT NULL,
    seasonal_tag             VARCHAR(128) NOT NULL,
    intensity                REAL NOT NULL CHECK (intensity >= 0.0 AND intensity <= 1.0),
    boost_symptoms           JSONB NOT NULL DEFAULT '[]'::jsonb,
    priority_question_hint   TEXT NOT NULL DEFAULT '',
    active                   BOOLEAN NOT NULL DEFAULT TRUE,
    valid_from               DATE NOT NULL,
    valid_to                 DATE NOT NULL,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (valid_from <= valid_to),
    PRIMARY KEY (tenant_id, context_id)
);

CREATE INDEX IF NOT EXISTS idx_epidemic_active_region_season
    ON epidemic_contexts (tenant_id, active, region_code, season, valid_from, valid_to);

CREATE TABLE IF NOT EXISTS symptom_synonyms (
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    standard_term            VARCHAR(128) NOT NULL,
    colloquial_terms         JSONB NOT NULL DEFAULT '[]'::jsonb,
    enabled                  BOOLEAN NOT NULL DEFAULT TRUE,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, standard_term)
);

CREATE INDEX IF NOT EXISTS idx_symptom_synonyms_tenant_enabled
    ON symptom_synonyms (tenant_id, enabled);

CREATE TABLE IF NOT EXISTS red_flag_rules (
    rule_id                  VARCHAR(128) NOT NULL,
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    level                    VARCHAR(32) NOT NULL CHECK (level IN ('None', 'Urgent', 'Emergency120')),
    trigger_all_of           JSONB NOT NULL DEFAULT '[]'::jsonb,
    trigger_any_of           JSONB NOT NULL DEFAULT '[]'::jsonb,
    alert_message            TEXT NOT NULL,
    enabled                  BOOLEAN NOT NULL DEFAULT TRUE,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    CHECK (jsonb_array_length(trigger_all_of) > 0 OR jsonb_array_length(trigger_any_of) > 0),
    PRIMARY KEY (tenant_id, rule_id)
);

CREATE INDEX IF NOT EXISTS idx_red_flag_rules_tenant_enabled
    ON red_flag_rules (tenant_id, enabled);

CREATE TABLE IF NOT EXISTS triage_audit_logs (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    session_id               VARCHAR(128) NOT NULL,
    patient_id               VARCHAR(128) NOT NULL,
    red_flag                 VARCHAR(32) NOT NULL CHECK (red_flag IN ('None', 'Urgent', 'Emergency120')),
    triage_status            VARCHAR(32) NOT NULL,
    recommended_department   VARCHAR(128),
    urgency_level            SMALLINT CHECK (urgency_level IS NULL OR urgency_level BETWEEN 1 AND 4),
    provider_id              VARCHAR(128),
    model_name               VARCHAR(128),
    prompt_tokens            INTEGER NOT NULL DEFAULT 0,
    completion_tokens        INTEGER NOT NULL DEFAULT 0,
    latency_ms               BIGINT NOT NULL DEFAULT 0,
    seasonal_contexts_applied JSONB NOT NULL DEFAULT '[]'::jsonb,
    citation_doc_ids         JSONB NOT NULL DEFAULT '[]'::jsonb,
    error_message            TEXT,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_audit_logs_tenant_created
    ON triage_audit_logs (tenant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_audit_logs_session
    ON triage_audit_logs (tenant_id, session_id);
CREATE INDEX IF NOT EXISTS idx_audit_logs_patient
    ON triage_audit_logs (tenant_id, patient_id, created_at DESC);

CREATE TABLE IF NOT EXISTS api_keys (
    key_id                   VARCHAR(64) PRIMARY KEY,
    tenant_id                VARCHAR(64) NOT NULL,
    name                     VARCHAR(128) NOT NULL,
    key_prefix               VARCHAR(32) NOT NULL,
    key_hash                 VARCHAR(128) NOT NULL UNIQUE,
    qpm_limit                INTEGER NOT NULL DEFAULT 120 CHECK (qpm_limit > 0),
    tpm_limit                INTEGER NOT NULL DEFAULT 100000 CHECK (tpm_limit > 0),
    namespace                VARCHAR(128) NOT NULL DEFAULT 'default',
    revoked                  BOOLEAN NOT NULL DEFAULT FALSE,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at             TIMESTAMPTZ,
    revoked_at               TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_api_keys_tenant_revoked
    ON api_keys (tenant_id, revoked);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash_active
    ON api_keys (key_hash)
    WHERE revoked = FALSE;

CREATE TABLE IF NOT EXISTS llm_providers (
    id                       VARCHAR(128) NOT NULL,
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    name                     VARCHAR(128) NOT NULL,
    kind                     VARCHAR(32) NOT NULL CHECK (kind IN ('openai_compatible', 'ollama', 'vllm', 'deepseek', 'qwen', 'gemini_openai', 'custom_http')),
    base_url                 TEXT NOT NULL,
    encrypted_api_key        BYTEA NOT NULL,
    encryption_nonce         BYTEA NOT NULL CHECK (octet_length(encryption_nonce) = 12),
    key_version              INTEGER NOT NULL DEFAULT 1,
    model                    VARCHAR(128) NOT NULL,
    tier                     VARCHAR(32) NOT NULL CHECK (tier IN ('fast', 'reasoning')),
    timeout_ms               BIGINT NOT NULL DEFAULT 8000 CHECK (timeout_ms > 0),
    is_intranet              BOOLEAN NOT NULL DEFAULT FALSE,
    enabled                  BOOLEAN NOT NULL DEFAULT TRUE,
    priority                 INTEGER NOT NULL DEFAULT 100,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (tenant_id, id)
);

CREATE INDEX IF NOT EXISTS idx_llm_providers_routing
    ON llm_providers (tenant_id, tier, enabled, priority ASC);

CREATE TABLE IF NOT EXISTS llm_router_config (
    tenant_id                VARCHAR(64) PRIMARY KEY DEFAULT 'default',
    config_json              JSONB NOT NULL,
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS ingestion_checkpoints (
    job_id                   VARCHAR(128) PRIMARY KEY,
    tenant_id                VARCHAR(64) NOT NULL DEFAULT 'default',
    source_path              TEXT NOT NULL,
    job_type                 VARCHAR(32) NOT NULL CHECK (job_type IN ('knowledge', 'knowledge_jsonl', 'knowledge_book', 'knowledge_batch', 'patient', 'patient_records')),
    status                   VARCHAR(32) NOT NULL CHECK (status IN ('pending', 'running', 'paused', 'completed', 'failed')),
    total_items              BIGINT NOT NULL DEFAULT 0,
    processed_items          BIGINT NOT NULL DEFAULT 0,
    failed_items             BIGINT NOT NULL DEFAULT 0,
    byte_offset              BIGINT NOT NULL DEFAULT 0,
    last_doc_id              TEXT,
    error_samples            JSONB NOT NULL DEFAULT '[]'::jsonb,
    started_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at             TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_ingestion_checkpoints_tenant_status
    ON ingestion_checkpoints (tenant_id, status, updated_at DESC);
