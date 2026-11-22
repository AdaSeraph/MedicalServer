use std::path::Path;

use medical_core::crypto::{EncryptedEnvelope, FieldCipher};
use medical_core::llm::{EmbeddingProvider, ProviderConfig, RouterConfig};
use medical_core::models::{EpidemicContext, MedicalDictionaries, PatientRecord};
use medical_core::storage::CoreStores;
use serde::{Deserialize, Serialize};

/// AES-256-GCM encrypted local state snapshot shared across `medical-server`,
/// `medical-studio`, and `medical-mcp --standalone`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalServerSnapshot {
    pub version: u32,
    pub patients: Vec<PatientRecord>,
    pub epidemics: Vec<EpidemicContext>,
    pub dictionaries: MedicalDictionaries,
    pub llm_providers: Vec<ProviderConfig>,
    pub router_config: RouterConfig,
}

/// Captures full runtime state and writes an AES-256-GCM encrypted file atomically
/// (temp file + rename) so abrupt termination never corrupts persisted data.
pub async fn save_local_snapshot(
    stores: &CoreStores,
    cipher: &FieldCipher,
    path: &Path,
) -> Result<usize, String> {
    let mut patients = Vec::new();
    let pids = stores
        .patient_memory
        .list_patient_ids(10_000, 0)
        .await
        .map_err(|e| e.to_string())?;
    for pid in pids {
        if let Ok(Some(rec)) = stores.patient_memory.get_full_memory(&pid, 200).await {
            patients.push(rec);
        }
    }

    let epidemics = stores
        .epidemics
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    let dictionaries = stores
        .dictionaries
        .load_dictionaries()
        .await
        .map_err(|e| e.to_string())?;
    let llm_providers = stores
        .llm_providers
        .list_providers()
        .await
        .map_err(|e| e.to_string())?;
    let router_config = stores
        .llm_providers
        .get_router_config()
        .await
        .map_err(|e| e.to_string())?;

    let snapshot = LocalServerSnapshot {
        version: 1,
        patients,
        epidemics,
        dictionaries,
        llm_providers,
        router_config,
    };

    let envelope = cipher
        .encrypt_json(&snapshot)
        .map_err(|e| format!("encrypt local snapshot: {e}"))?;
    let encoded = envelope.to_compact_base64();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create snapshot dir {}: {e}", parent.display()))?;
    }
    let tmp_path = path.with_extension("enc.tmp");
    std::fs::write(&tmp_path, encoded.as_bytes())
        .map_err(|e| format!("write temp snapshot {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("commit snapshot {}: {e}", path.display()))?;

    Ok(snapshot.patients.len() + snapshot.epidemics.len() + snapshot.llm_providers.len())
}

/// Restores a previously saved AES-256-GCM encrypted snapshot into `CoreStores`.
pub async fn load_local_snapshot(
    stores: &CoreStores,
    cipher: &FieldCipher,
    embedder: &dyn EmbeddingProvider,
    path: &Path,
) -> Result<bool, String> {
    if !path.is_file() {
        return Ok(false);
    }
    let encoded = std::fs::read_to_string(path)
        .map_err(|e| format!("read local snapshot {}: {e}", path.display()))?;
    let envelope = EncryptedEnvelope::from_compact_base64(&encoded)
        .map_err(|e| format!("parse local snapshot envelope: {e}"))?;
    let snapshot: LocalServerSnapshot = cipher
        .decrypt_json(&envelope)
        .map_err(|e| format!("decrypt local snapshot: {e}"))?;

    for rec in snapshot.patients {
        let _ = stores
            .patient_memory
            .upsert_l2_profile(
                &rec.patient_id,
                rec.consent_long_term_memory,
                &rec.l2_profile,
            )
            .await;
        for ev in &rec.l3_visit_events {
            let _ = stores
                .patient_memory
                .insert_l3_event(&rec.patient_id, ev)
                .await;
            let embed_text = format!(
                "{} {} {}",
                ev.chief_complaint,
                ev.summary,
                ev.clinical_exams.to_searchable_text()
            );
            if let Ok(vec) = embedder.embed_one(&embed_text).await {
                let _ = stores
                    .patient_l3_vectors
                    .upsert_event_vector(&rec.patient_id, ev, vec)
                    .await;
            }
        }
    }

    for epi in snapshot.epidemics {
        let _ = stores.epidemics.upsert(&epi).await;
    }

    for syn in snapshot.dictionaries.symptom_synonyms {
        let _ = stores.dictionaries.save_symptom_synonym(&syn).await;
    }
    for rule in snapshot.dictionaries.red_flag_rules {
        let _ = stores.dictionaries.save_red_flag_rule(&rule).await;
    }

    for p in snapshot.llm_providers {
        let _ = stores.llm_providers.upsert_provider(&p).await;
    }
    let _ = stores
        .llm_providers
        .save_router_config(&snapshot.router_config)
        .await;

    Ok(true)
}
