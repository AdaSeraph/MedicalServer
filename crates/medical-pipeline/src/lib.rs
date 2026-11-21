pub mod book_chunker;
pub mod checkpoint;
pub mod error;
pub mod indexer;
pub mod json_stream;
pub mod patient_import;

pub use book_chunker::{
    BookMetadataDefaults, CustomChunkRule, NoopOcrPlugin, OcrPlugin, parse_docx_book,
    parse_markdown_book, parse_medical_book_file, parse_pdf_book, parse_plain_text_book,
};
pub use checkpoint::{CheckpointManager, JobProgress, export_error_samples_jsonl};
pub use error::{PipelineError, PipelineResult};
pub use indexer::{
    DEFAULT_SEASONAL_ALPHA, DualWriteIndexer, HybridKnowledgeSearchEngine, TantivyKnowledgeIndex,
};
pub use json_stream::{
    JsonlLineError, JsonlStreamReport, count_jsonl_lines, parse_collection_bundle_file,
    parse_collection_bundle_str, stream_knowledge_jsonl, stream_patient_jsonl,
    stream_validated_jsonl, strip_bom_and_crlf,
};
pub use patient_import::{
    PatientImportReport, PatientMaskingConfig, PiiColumnDetection, bucket_age_group,
    detect_pii_columns, import_patients_csv, import_patients_file, import_patients_json,
    import_patients_xlsx,
};
