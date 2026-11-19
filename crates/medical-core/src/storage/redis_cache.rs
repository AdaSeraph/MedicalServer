use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use tokio::sync::OnceCell;

use super::StorageResult;
use super::traits::{SessionCache, SessionState};

pub struct RedisSessionCache {
    client: redis::Client,
    manager: OnceCell<ConnectionManager>,
}

impl RedisSessionCache {
    pub fn from_url(redis_url: &str) -> StorageResult<Self> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self {
            client,
            manager: OnceCell::new(),
        })
    }

    async fn conn(&self) -> StorageResult<ConnectionManager> {
        let mgr = self
            .manager
            .get_or_try_init(|| async { ConnectionManager::new(self.client.clone()).await })
            .await?;
        Ok(mgr.clone())
    }

    fn session_key(session_id: &str) -> String {
        format!("triage:sess:{session_id}")
    }

    fn patient_set_key(patient_id: &str) -> String {
        format!("triage:pat_sess:{patient_id}")
    }

    fn semantic_key(cache_key: &str) -> String {
        format!("triage:sem:{cache_key}")
    }
}

#[async_trait]
impl SessionCache for RedisSessionCache {
    async fn get_session(&self, session_id: &str) -> StorageResult<Option<SessionState>> {
        let mut con = self.conn().await?;
        let raw: Option<String> = con.get(Self::session_key(session_id)).await?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    async fn save_session(&self, session: &SessionState, ttl_secs: u64) -> StorageResult<()> {
        let mut con = self.conn().await?;
        let ttl = ttl_secs.max(1);
        let payload = serde_json::to_string(session)?;
        let sess_key = Self::session_key(&session.session_id);
        let pat_key = Self::patient_set_key(&session.patient_id);

        con.set_ex::<_, _, ()>(&sess_key, &payload, ttl).await?;
        con.sadd::<_, _, ()>(&pat_key, &session.session_id).await?;
        con.expire::<_, ()>(&pat_key, ttl as i64).await?;
        Ok(())
    }

    async fn delete_session(&self, session_id: &str) -> StorageResult<bool> {
        let mut con = self.conn().await?;
        let deleted: u64 = con.del(Self::session_key(session_id)).await?;
        Ok(deleted > 0)
    }

    async fn delete_patient_sessions(&self, patient_id: &str) -> StorageResult<u64> {
        let mut con = self.conn().await?;
        let pat_key = Self::patient_set_key(patient_id);
        let session_ids: Vec<String> = con.smembers(&pat_key).await?;
        let mut keys: Vec<String> = session_ids.iter().map(|s| Self::session_key(s)).collect();
        keys.push(pat_key);
        let deleted: u64 = con.del(keys).await?;
        Ok(deleted.saturating_sub(1).min(session_ids.len() as u64))
    }

    async fn get_cached_response(&self, cache_key: &str) -> StorageResult<Option<String>> {
        let mut con = self.conn().await?;
        let raw: Option<String> = con.get(Self::semantic_key(cache_key)).await?;
        Ok(raw)
    }

    async fn set_cached_response(
        &self,
        cache_key: &str,
        payload: &str,
        ttl_secs: u64,
    ) -> StorageResult<()> {
        let mut con = self.conn().await?;
        con.set_ex::<_, _, ()>(Self::semantic_key(cache_key), payload, ttl_secs.max(1))
            .await?;
        Ok(())
    }
}
