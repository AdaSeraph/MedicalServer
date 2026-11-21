use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub bind_addr: String,
    pub offline_mode: bool,
    pub jwt_secret: String,
    pub jwt_ttl_secs: u64,
    pub patient_id_salt: Vec<u8>,
    pub field_encryption_key: String,
    pub admin_token: String,
    pub wechat_app_id: Option<String>,
    pub wechat_app_secret: Option<String>,
    pub wechat_api_base: String,
    pub database_url: Option<String>,
    pub redis_url: Option<String>,
    pub qdrant_url: Option<String>,
    pub openai_base_url: Option<String>,
    pub openai_api_key: String,
    pub openai_fast_model: String,
    pub openai_reasoning_model: String,
    pub embedding_base_url: Option<String>,
    pub rerank_base_url: Option<String>,
    pub tenant_keys_file: Option<PathBuf>,
    pub seed_corpus_file: Option<PathBuf>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "127.0.0.1:8080".to_string(),
            offline_mode: true,
            jwt_secret: "medical-server-dev-jwt-secret-change-in-prod".to_string(),
            jwt_ttl_secs: 7200,
            patient_id_salt: b"medical-server-hospital-salt-v1".to_vec(),
            field_encryption_key: "medical-server-dev-field-key-v1".to_string(),
            admin_token: "e2e-admin-secret".to_string(),
            wechat_app_id: None,
            wechat_app_secret: None,
            wechat_api_base: "https://api.weixin.qq.com".to_string(),
            database_url: None,
            redis_url: None,
            qdrant_url: None,
            openai_base_url: None,
            openai_api_key: "sk-offline-mock".to_string(),
            openai_fast_model: "medical-fast".to_string(),
            openai_reasoning_model: "medical-reasoning".to_string(),
            embedding_base_url: None,
            rerank_base_url: None,
            tenant_keys_file: None,
            seed_corpus_file: None,
        }
    }
}

impl ServerConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        if let Ok(addr) = env::var("MEDICAL_SERVER_ADDR")
            .or_else(|_| env::var("SERVER_ADDR"))
            .map(|s| s.trim().to_string())
            && !addr.is_empty()
        {
            cfg.bind_addr = addr;
        } else if let Ok(port) = env::var("PORT")
            && !port.trim().is_empty()
        {
            cfg.bind_addr = format!("0.0.0.0:{}", port.trim());
        }

        cfg.database_url = env::var("DATABASE_URL").ok().filter(|s| !s.trim().is_empty());
        cfg.redis_url = env::var("REDIS_URL").ok().filter(|s| !s.trim().is_empty());
        cfg.qdrant_url = env::var("QDRANT_URL").ok().filter(|s| !s.trim().is_empty());

        let force_offline = env::var("MEDICAL_R5_OFFLINE")
            .or_else(|_| env::var("USE_IN_MEMORY_ADAPTERS"))
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        cfg.offline_mode = force_offline || cfg.database_url.is_none();

        if let Ok(secret) = env::var("JWT_SECRET")
            && !secret.trim().is_empty()
        {
            cfg.jwt_secret = secret;
        }
        if let Ok(ttl) = env::var("JWT_TTL_SECS")
            && let Ok(secs) = ttl.parse::<u64>()
        {
            cfg.jwt_ttl_secs = secs.max(60);
        }
        if let Ok(salt) = env::var("PATIENT_ID_SALT")
            && !salt.trim().is_empty()
        {
            cfg.patient_id_salt = salt.into_bytes();
        }
        if let Ok(key) = env::var("FIELD_ENCRYPTION_KEY")
            && !key.trim().is_empty()
        {
            cfg.field_encryption_key = key;
        }
        if let Ok(tok) = env::var("MEDICAL_ADMIN_TOKEN")
            && !tok.trim().is_empty()
        {
            cfg.admin_token = tok;
        }

        cfg.wechat_app_id = env::var("WECHAT_APP_ID").ok().filter(|s| !s.trim().is_empty());
        cfg.wechat_app_secret = env::var("WECHAT_APP_SECRET")
            .ok()
            .filter(|s| !s.trim().is_empty());
        if let Ok(base) = env::var("WECHAT_API_BASE_URL")
            && !base.trim().is_empty()
        {
            cfg.wechat_api_base = base;
        }

        cfg.openai_base_url = env::var("OPENAI_BASE_URL")
            .or_else(|_| env::var("LLM_BASE_URL"))
            .ok()
            .filter(|s| !s.trim().is_empty());
        if let Ok(api_key) = env::var("OPENAI_API_KEY")
            && !api_key.trim().is_empty()
        {
            cfg.openai_api_key = api_key;
        }
        if let Ok(m) = env::var("OPENAI_FAST_MODEL")
            && !m.trim().is_empty()
        {
            cfg.openai_fast_model = m;
        }
        if let Ok(m) = env::var("OPENAI_REASONING_MODEL")
            && !m.trim().is_empty()
        {
            cfg.openai_reasoning_model = m;
        }
        cfg.embedding_base_url = env::var("EMBEDDING_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());
        cfg.rerank_base_url = env::var("RERANK_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());

        cfg.tenant_keys_file = env::var("MEDICAL_TENANT_KEYS_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);
        cfg.seed_corpus_file = env::var("MEDICAL_SEED_CORPUS_FILE")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from);

        cfg
    }
}
