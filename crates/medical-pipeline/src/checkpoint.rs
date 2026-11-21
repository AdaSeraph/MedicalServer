use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use medical_core::storage::{CheckpointStatus, CheckpointStore, IngestionCheckpoint};
use serde::{Deserialize, Serialize};

use crate::error::{PipelineError, PipelineResult};

/// Telemetry snapshot for an ingestion job (used by CLI, Tauri Studio, and tests).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobProgress {
    pub job_id: String,
    pub source_path: String,
    pub job_type: String,
    pub status: CheckpointStatus,
    pub total_items: u64,
    pub processed_items: u64,
    pub failed_items: u64,
    pub byte_offset: u64,
    pub last_doc_id: Option<String>,
    pub throughput_items_per_sec: f64,
    pub elapsed_ms: u64,
    pub completed: bool,
    pub error_samples: Vec<String>,
}

/// Manages resumable ingestion checkpoints (断点续传) and progress telemetry.
pub struct CheckpointManager {
    store: Arc<dyn CheckpointStore>,
    max_error_samples: usize,
}

impl CheckpointManager {
    pub fn new(store: Arc<dyn CheckpointStore>) -> Self {
        Self {
            store,
            max_error_samples: 50,
        }
    }

    pub fn with_max_error_samples(mut self, max_error_samples: usize) -> Self {
        self.max_error_samples = max_error_samples;
        self
    }

    /// Loads an existing checkpoint or initializes a fresh pending checkpoint.
    pub async fn load_or_init(
        &self,
        job_id: &str,
        source_path: &str,
        job_type: &str,
        total_items: u64,
    ) -> PipelineResult<IngestionCheckpoint> {
        if let Some(existing) = self.store.get_checkpoint(job_id).await? {
            return Ok(existing);
        }
        let cp = IngestionCheckpoint {
            job_id: job_id.to_string(),
            source_path: source_path.to_string(),
            job_type: job_type.to_string(),
            status: CheckpointStatus::Pending,
            total_items,
            processed_items: 0,
            failed_items: 0,
            byte_offset: 0,
            last_doc_id: None,
            error_samples: Vec::new(),
            updated_at: Utc::now(),
        };
        self.store.save_checkpoint(&cp).await?;
        Ok(cp)
    }

    pub async fn save_progress(
        &self,
        cp: &mut IngestionCheckpoint,
        status: CheckpointStatus,
    ) -> PipelineResult<()> {
        cp.status = status;
        cp.updated_at = Utc::now();
        if cp.error_samples.len() > self.max_error_samples {
            cp.error_samples.truncate(self.max_error_samples);
        }
        self.store.save_checkpoint(cp).await?;
        Ok(())
    }

    pub async fn get(&self, job_id: &str) -> PipelineResult<Option<IngestionCheckpoint>> {
        Ok(self.store.get_checkpoint(job_id).await?)
    }

    pub async fn list(&self, limit: usize) -> PipelineResult<Vec<IngestionCheckpoint>> {
        Ok(self.store.list_checkpoints(limit).await?)
    }

    pub async fn reset(&self, job_id: &str) -> PipelineResult<bool> {
        Ok(self.store.reset_checkpoint(job_id).await?)
    }

    pub fn build_progress(cp: &IngestionCheckpoint, started_at: Instant) -> JobProgress {
        let elapsed_ms = started_at.elapsed().as_millis() as u64;
        let elapsed_secs = (elapsed_ms as f64 / 1000.0).max(0.001);
        let completed_count = cp.processed_items.saturating_add(cp.failed_items);
        let throughput = completed_count as f64 / elapsed_secs;
        JobProgress {
            job_id: cp.job_id.clone(),
            source_path: cp.source_path.clone(),
            job_type: cp.job_type.clone(),
            status: cp.status,
            total_items: cp.total_items,
            processed_items: cp.processed_items,
            failed_items: cp.failed_items,
            byte_offset: cp.byte_offset,
            last_doc_id: cp.last_doc_id.clone(),
            throughput_items_per_sec: throughput,
            elapsed_ms,
            completed: cp.status == CheckpointStatus::Completed,
            error_samples: cp.error_samples.clone(),
        }
    }
}

/// Exports failed ingestion error records as JSONL for operator inspection.
pub fn export_error_samples_jsonl(
    errors: &[String],
    output_path: impl AsRef<Path>,
) -> PipelineResult<usize> {
    let file = File::create(output_path.as_ref()).map_err(PipelineError::Io)?;
    let mut writer = BufWriter::new(file);
    for (idx, msg) in errors.iter().enumerate() {
        let row = serde_json::json!({
            "index": idx + 1,
            "error": msg,
        });
        writeln!(writer, "{row}")?;
    }
    writer.flush()?;
    Ok(errors.len())
}
