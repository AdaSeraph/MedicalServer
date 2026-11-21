use axum::http::{HeaderMap, StatusCode, header};
use chrono::Utc;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use medical_core::storage::ApiKeyRecord;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    pub sub: String,
    pub role: String,
    pub tenant_id: String,
    pub namespace: String,
    pub iat: usize,
    pub exp: usize,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub subject: String,
    pub patient_id: Option<String>,
    pub tenant_id: String,
    pub namespace: String,
    pub api_key: Option<ApiKeyRecord>,
    pub is_admin: bool,
}

#[derive(Debug)]
pub struct AuthError {
    pub status: StatusCode,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl AuthError {
    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: msg.into(),
            retry_after_secs: None,
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
            retry_after_secs: None,
        }
    }

    pub fn rate_limited(msg: impl Into<String>, retry_after_secs: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: msg.into(),
            retry_after_secs: Some(retry_after_secs),
        }
    }
}

pub fn issue_wechat_jwt(
    secret: &str,
    patient_id: &str,
    ttl_secs: u64,
) -> Result<(String, u64), AuthError> {
    let now = Utc::now().timestamp().max(0) as usize;
    let exp = now + ttl_secs as usize;
    let claims = JwtClaims {
        sub: patient_id.to_string(),
        role: "patient".to_string(),
        tenant_id: "wechat_miniapp".to_string(),
        namespace: "default".to_string(),
        iat: now,
        exp,
    };
    let enc_key = EncodingKey::from_secret(secret.as_bytes())
        .map_err(|e| AuthError::unauthorized(format!("jwt key error: {e}")))?;
    let token = encode(&Header::default(), &claims, &enc_key)
        .map_err(|e| AuthError::unauthorized(format!("jwt sign error: {e}")))?;
    Ok((token, ttl_secs))
}

pub fn verify_jwt(secret: &str, token: &str) -> Result<JwtClaims, AuthError> {
    let dec_key = DecodingKey::from_secret(secret.as_bytes())
        .map_err(|e| AuthError::unauthorized(format!("jwt key error: {e}")))?;
    let data = decode::<JwtClaims>(token, &dec_key, &Validation::default())
        .map_err(|e| AuthError::unauthorized(format!("invalid jwt: {e}")))?;
    Ok(data.claims)
}

pub fn extract_bearer_token(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let trimmed = raw.trim();
    trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Authenticates a request via Admin token, multi-tenant `mk_live_*` API key (with QPM/TPM enforcement),
/// or WeChat patient JWT.
pub async fn authenticate_request(
    state: &AppState,
    headers: &HeaderMap,
    estimated_tokens: u32,
) -> Result<AuthContext, AuthError> {
    let Some(token) = extract_bearer_token(headers) else {
        return Err(AuthError::unauthorized(
            "missing Authorization: Bearer <token> header",
        ));
    };

    // 1. Admin bearer token
    if !state.config.admin_token.is_empty() && token == state.config.admin_token {
        return Ok(AuthContext {
            subject: "admin".to_string(),
            patient_id: None,
            tenant_id: "admin".to_string(),
            namespace: "default".to_string(),
            api_key: None,
            is_admin: true,
        });
    }

    // 2. Multi-tenant API key (mk_live_* / mk_test_*)
    if token.starts_with("mk_") {
        let record = state
            .stores
            .api_keys
            .verify_key(token)
            .await
            .map_err(|e| AuthError::unauthorized(format!("api key lookup error: {e}")))?
            .ok_or_else(|| AuthError::unauthorized("invalid or revoked API key"))?;

        if record.revoked {
            return Err(AuthError::unauthorized("API key has been revoked"));
        }

        if let Err(rej) = state.rate_limiter.check_and_consume(
            &record.key_id,
            record.qpm_limit,
            record.tpm_limit,
            estimated_tokens,
        ) {
            return Err(AuthError::rate_limited(
                format!("tenant {} rate limit exceeded ({})", record.tenant_id, rej.kind),
                rej.retry_after_secs,
            ));
        }

        return Ok(AuthContext {
            subject: record.tenant_id.clone(),
            patient_id: None,
            tenant_id: record.tenant_id.clone(),
            namespace: record.namespace.clone(),
            api_key: Some(record),
            is_admin: false,
        });
    }

    // 3. WeChat patient JWT
    let claims = verify_jwt(&state.config.jwt_secret, token)?;
    Ok(AuthContext {
        subject: claims.sub.clone(),
        patient_id: Some(claims.sub),
        tenant_id: claims.tenant_id,
        namespace: claims.namespace,
        api_key: None,
        is_admin: claims.role == "admin",
    })
}
