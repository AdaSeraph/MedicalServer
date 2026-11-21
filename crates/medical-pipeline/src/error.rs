use thiserror::Error;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("storage error: {0}")]
    Storage(#[from] medical_core::storage::StorageError),

    #[error("embedding error: {0}")]
    Embedding(#[from] medical_core::llm::LlmError),

    #[error("index error: {0}")]
    Index(String),

    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    #[error("scanned PDF has no extractable text layer and requires OCR plugin")]
    ScannedPdfNeedsOcr,

    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
}

pub type PipelineResult<T> = Result<T, PipelineError>;
