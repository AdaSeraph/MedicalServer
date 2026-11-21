use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use medical_core::models::{DataCollectionPackage, KnowledgeDocument, PatientRecord};
use medical_core::validation::Validate;
use serde::{Deserialize, Serialize};

use crate::error::{PipelineError, PipelineResult};

/// A single line-level error captured while streaming JSONL without aborting the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonlLineError {
    pub line_number: usize,
    pub byte_offset: u64,
    pub message: String,
}

/// Result of streaming a JSONL file from `start_offset`.
#[derive(Debug, Clone)]
pub struct JsonlStreamReport<T> {
    pub records: Vec<T>,
    pub errors: Vec<JsonlLineError>,
    pub lines_read: usize,
    pub end_byte_offset: u64,
}

/// Strips a leading UTF-8 BOM (`\u{feff}`) and trailing `\r`/`\n` characters.
#[inline]
pub fn strip_bom_and_crlf(raw: &str) -> &str {
    let without_bom = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    without_bom.trim_end_matches(['\r', '\n'])
}

/// Parses and validates a complete `DataCollectionPackage` JSON string using `sonic-rs`.
pub fn parse_collection_bundle_str(raw: &str) -> PipelineResult<DataCollectionPackage> {
    let cleaned = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let pkg: DataCollectionPackage = sonic_rs::from_str(cleaned)
        .map_err(|e| PipelineError::Parse(format!("sonic-rs JSON bundle parse error: {e}")))?;
    pkg.validate()
        .map_err(|e| PipelineError::Validation(e.to_string()))?;
    Ok(pkg)
}

/// Parses and validates a complete `DataCollectionPackage` JSON file using `sonic-rs`.
pub fn parse_collection_bundle_file(path: impl AsRef<Path>) -> PipelineResult<DataCollectionPackage> {
    let raw = std::fs::read_to_string(path.as_ref())?;
    parse_collection_bundle_str(&raw)
}

/// Streams a JSONL file of `KnowledgeDocument`s using `BufReader` + `sonic-rs`.
///
/// - Handles UTF-8 BOM on line 1 and Windows CRLF line endings.
/// - Supports resuming from `start_byte_offset` (断点续传).
/// - Validates every record via `medical_core::validation::Validate`.
/// - Collects malformed lines in `errors` without aborting valid records.
pub fn stream_knowledge_jsonl(
    path: impl AsRef<Path>,
    start_byte_offset: u64,
    max_records: Option<usize>,
) -> PipelineResult<JsonlStreamReport<KnowledgeDocument>> {
    stream_validated_jsonl::<KnowledgeDocument>(path, start_byte_offset, max_records)
}

/// Streams a JSONL file of `PatientRecord`s using `BufReader` + `sonic-rs`.
pub fn stream_patient_jsonl(
    path: impl AsRef<Path>,
    start_byte_offset: u64,
    max_records: Option<usize>,
) -> PipelineResult<JsonlStreamReport<PatientRecord>> {
    stream_validated_jsonl::<PatientRecord>(path, start_byte_offset, max_records)
}

/// Generic streaming JSONL parser backed by `BufReader` and `sonic_rs::from_str`.
pub fn stream_validated_jsonl<T>(
    path: impl AsRef<Path>,
    start_byte_offset: u64,
    max_records: Option<usize>,
) -> PipelineResult<JsonlStreamReport<T>>
where
    T: serde::de::DeserializeOwned + Validate,
{
    let mut file = File::open(path.as_ref())?;
    if start_byte_offset > 0 {
        file.seek(SeekFrom::Start(start_byte_offset))?;
    }
    let mut reader = BufReader::new(file);
    let mut line_buf = String::new();
    let mut offset = start_byte_offset;
    let mut line_number = 0usize;

    let mut records = Vec::new();
    let mut errors = Vec::new();

    loop {
        if let Some(limit) = max_records
            && records.len() >= limit
        {
            break;
        }
        line_buf.clear();
        let bytes_read = reader.read_line(&mut line_buf)?;
        if bytes_read == 0 {
            break;
        }
        let line_start_offset = offset;
        offset = offset.saturating_add(bytes_read as u64);
        line_number += 1;

        let trimmed = strip_bom_and_crlf(&line_buf).trim();
        if trimmed.is_empty() {
            continue;
        }

        match sonic_rs::from_str::<T>(trimmed) {
            Ok(item) => match item.validate() {
                Ok(()) => records.push(item),
                Err(verr) => errors.push(JsonlLineError {
                    line_number,
                    byte_offset: line_start_offset,
                    message: format!("validation failed: {verr}"),
                }),
            },
            Err(perr) => errors.push(JsonlLineError {
                line_number,
                byte_offset: line_start_offset,
                message: format!("sonic-rs parse failed: {perr}"),
            }),
        }
    }

    Ok(JsonlStreamReport {
        records,
        errors,
        lines_read: line_number,
        end_byte_offset: offset,
    })
}

/// Counts non-empty lines in a JSONL file for progress total estimation.
pub fn count_jsonl_lines(path: impl AsRef<Path>) -> PipelineResult<u64> {
    let file = File::open(path.as_ref())?;
    let reader = BufReader::new(file);
    let mut count = 0u64;
    for line in reader.lines() {
        let line = line?;
        if !strip_bom_and_crlf(&line).trim().is_empty() {
            count += 1;
        }
    }
    Ok(count)
}
