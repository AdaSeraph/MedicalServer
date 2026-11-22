use std::collections::HashMap;
use std::path::Path;

use chrono::Utc;
use docx_rs::{DocumentChild, ParagraphChild, RunChild};
use medical_core::crypto::hash_api_key;
use medical_core::models::{KnowledgeDocument, SectionType, SourceType, TargetPopulation};
use medical_core::validation::Validate;
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{PipelineError, PipelineResult};

/// Optional OCR plugin hook for scanned / image-only PDFs.
pub trait OcrPlugin: Send + Sync {
    fn extract_text(&self, pdf_bytes: &[u8]) -> PipelineResult<String>;
}

/// Default no-op OCR plugin that signals a scanned PDF requires OCR.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopOcrPlugin;

impl OcrPlugin for NoopOcrPlugin {
    fn extract_text(&self, _pdf_bytes: &[u8]) -> PipelineResult<String> {
        Err(PipelineError::ScannedPdfNeedsOcr)
    }
}

/// Mandatory metadata defaults injected into every semantic chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookMetadataDefaults {
    pub source_title: String,
    pub source_type: SourceType,
    pub authority_level: f32,
    pub default_department: String,
    pub default_population: Vec<TargetPopulation>,
    #[serde(default)]
    pub seasonal_tags: Vec<String>,
}

impl Default for BookMetadataDefaults {
    fn default() -> Self {
        Self {
            source_title: "医学知识手册".to_string(),
            source_type: SourceType::Textbook,
            authority_level: 0.9,
            default_department: "全科医学科".to_string(),
            default_population: vec![TargetPopulation::Adult],
            seasonal_tags: Vec::new(),
        }
    }
}

/// Custom regex chunking rule for plain-text medical books/manuals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomChunkRule {
    /// Regex with capture group `disease` or group 1 identifying a disease boundary.
    pub disease_pattern: String,
    /// Regex with capture group `section` or group 1 identifying a section heading.
    pub section_pattern: String,
}

impl Default for CustomChunkRule {
    fn default() -> Self {
        Self {
            disease_pattern: r"^【疾病】\s*(?P<disease>.+)$".to_string(),
            section_pattern: r"^【(?P<section>临床表现|鉴别诊断|急症指征|推荐科室|流行病学特征|流行病学|分诊规则)】\s*(?P<body>.*)$".to_string(),
        }
    }
}

/// Intermediate semantic tree node for a single disease entry.
#[derive(Debug, Clone)]
struct ParsedDiseaseTree {
    disease_name: String,
    sections: Vec<(String, String)>,
}

/// Parses a medical book file (`.md`, `.txt`, `.docx`, `.pdf`) into validated semantic chunks.
pub fn parse_medical_book_file(
    path: impl AsRef<Path>,
    defaults: &BookMetadataDefaults,
    custom_rule: Option<&CustomChunkRule>,
    ocr: Option<&dyn OcrPlugin>,
) -> PipelineResult<Vec<KnowledgeDocument>> {
    let path = path.as_ref();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "md" | "markdown" => {
            let content = std::fs::read_to_string(path)?;
            parse_markdown_book(&content, defaults)
        }
        "txt" => {
            let content = std::fs::read_to_string(path)?;
            parse_plain_text_book(&content, defaults, custom_rule)
        }
        "docx" => {
            let bytes = std::fs::read(path)?;
            parse_docx_book(&bytes, defaults, custom_rule)
        }
        "pdf" => {
            let bytes = std::fs::read(path)?;
            parse_pdf_book(&bytes, defaults, custom_rule, ocr)
        }
        other => Err(PipelineError::UnsupportedFormat(format!(
            "unsupported book extension: .{other}"
        ))),
    }
}

/// Parses a Markdown medical book using `pulldown-cmark` into semantic `KnowledgeDocument`s.
pub fn parse_markdown_book(
    markdown: &str,
    defaults: &BookMetadataDefaults,
) -> PipelineResult<Vec<KnowledgeDocument>> {
    let cleaned = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let parser = Parser::new(cleaned);

    let mut trees: Vec<ParsedDiseaseTree> = Vec::new();
    let mut current_disease: Option<ParsedDiseaseTree> = None;
    let mut current_section_title: Option<String> = None;
    let mut current_section_body = String::new();

    let mut in_heading: Option<HeadingLevel> = None;
    let mut heading_buf = String::new();

    let flush_section = |disease: &mut Option<ParsedDiseaseTree>,
                         sec_title: &mut Option<String>,
                         sec_body: &mut String| {
        if let (Some(d), Some(title)) = (disease.as_mut(), sec_title.take()) {
            let trimmed = sec_body.trim();
            if !trimmed.is_empty() {
                d.sections.push((title, trimmed.to_string()));
            }
        }
        sec_body.clear();
    };

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                in_heading = Some(level);
                heading_buf.clear();
            }
            Event::End(TagEnd::Heading(_)) => {
                let title = heading_buf.trim().to_string();
                match in_heading.take() {
                    Some(HeadingLevel::H1) => {
                        flush_section(
                            &mut current_disease,
                            &mut current_section_title,
                            &mut current_section_body,
                        );
                        if let Some(prev) = current_disease.take() {
                            trees.push(prev);
                        }
                        if !title.is_empty() {
                            current_disease = Some(ParsedDiseaseTree {
                                disease_name: title,
                                sections: Vec::new(),
                            });
                        }
                    }
                    Some(HeadingLevel::H2 | HeadingLevel::H3 | HeadingLevel::H4) => {
                        flush_section(
                            &mut current_disease,
                            &mut current_section_title,
                            &mut current_section_body,
                        );
                        if current_disease.is_none() && !title.is_empty() {
                            current_disease = Some(ParsedDiseaseTree {
                                disease_name: defaults.source_title.clone(),
                                sections: Vec::new(),
                            });
                        }
                        if !title.is_empty() {
                            current_section_title = Some(title);
                        }
                    }
                    _ => {}
                }
                heading_buf.clear();
            }
            Event::Text(text) | Event::Code(text) => {
                if in_heading.is_some() {
                    heading_buf.push_str(&text);
                } else {
                    if current_disease.is_none() {
                        current_disease = Some(ParsedDiseaseTree {
                            disease_name: defaults.source_title.clone(),
                            sections: Vec::new(),
                        });
                    }
                    if current_section_title.is_none() {
                        current_section_title = Some("临床表现".to_string());
                    }
                    current_section_body.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if in_heading.is_some() {
                    heading_buf.push(' ');
                } else {
                    current_section_body.push('\n');
                }
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => {
                if in_heading.is_none() && !current_section_body.ends_with('\n') {
                    current_section_body.push('\n');
                }
            }
            _ => {}
        }
    }

    flush_section(
        &mut current_disease,
        &mut current_section_title,
        &mut current_section_body,
    );
    if let Some(prev) = current_disease.take() {
        trees.push(prev);
    }

    build_chunks_from_trees(trees, defaults)
}

/// Parses a plain text (`.txt`) medical manual using custom or default regex markers.
pub fn parse_plain_text_book(
    text: &str,
    defaults: &BookMetadataDefaults,
    custom_rule: Option<&CustomChunkRule>,
) -> PipelineResult<Vec<KnowledgeDocument>> {
    let cleaned = text.strip_prefix('\u{feff}').unwrap_or(text);
    let default_rule = CustomChunkRule::default();
    let rule = custom_rule.unwrap_or(&default_rule);

    let disease_re = Regex::new(&rule.disease_pattern)
        .map_err(|e| PipelineError::Parse(format!("invalid disease_pattern regex: {e}")))?;
    let section_re = Regex::new(&rule.section_pattern)
        .map_err(|e| PipelineError::Parse(format!("invalid section_pattern regex: {e}")))?;

    let mut trees: Vec<ParsedDiseaseTree> = Vec::new();
    let mut current_disease: Option<ParsedDiseaseTree> = None;
    let mut current_section_title: Option<String> = None;
    let mut current_section_body = String::new();

    let flush_section = |disease: &mut Option<ParsedDiseaseTree>,
                         sec_title: &mut Option<String>,
                         sec_body: &mut String| {
        if let (Some(d), Some(title)) = (disease.as_mut(), sec_title.take()) {
            let trimmed = sec_body.trim();
            if !trimmed.is_empty() {
                d.sections.push((title, trimmed.to_string()));
            }
        }
        sec_body.clear();
    };

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(caps) = disease_re.captures(line) {
            flush_section(
                &mut current_disease,
                &mut current_section_title,
                &mut current_section_body,
            );
            if let Some(prev) = current_disease.take() {
                trees.push(prev);
            }
            let name = caps
                .name("disease")
                .or_else(|| caps.get(1))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| defaults.source_title.clone());
            current_disease = Some(ParsedDiseaseTree {
                disease_name: name,
                sections: Vec::new(),
            });
            continue;
        }

        if let Some(caps) = section_re.captures(line) {
            flush_section(
                &mut current_disease,
                &mut current_section_title,
                &mut current_section_body,
            );
            if current_disease.is_none() {
                current_disease = Some(ParsedDiseaseTree {
                    disease_name: defaults.source_title.clone(),
                    sections: Vec::new(),
                });
            }
            let sec = caps
                .name("section")
                .or_else(|| caps.get(1))
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_else(|| "临床表现".to_string());
            current_section_title = Some(sec);
            if let Some(body) = caps.name("body").or_else(|| caps.get(2)) {
                let body_str = body.as_str().trim();
                if !body_str.is_empty() {
                    current_section_body.push_str(body_str);
                }
            }
            continue;
        }

        if current_disease.is_none() {
            current_disease = Some(ParsedDiseaseTree {
                disease_name: defaults.source_title.clone(),
                sections: Vec::new(),
            });
        }
        if current_section_title.is_none() {
            current_section_title = Some("临床表现".to_string());
        }
        if !current_section_body.is_empty() {
            current_section_body.push('\n');
        }
        current_section_body.push_str(line);
    }

    flush_section(
        &mut current_disease,
        &mut current_section_title,
        &mut current_section_body,
    );
    if let Some(prev) = current_disease.take() {
        trees.push(prev);
    }

    build_chunks_from_trees(trees, defaults)
}

/// Extracts plain text (preserving Word Heading1/Heading2 as Markdown `#`/`##`) from `.docx` bytes.
pub fn extract_docx_text(bytes: &[u8]) -> PipelineResult<String> {
    let docx = docx_rs::read_docx(bytes)
        .map_err(|e| PipelineError::Parse(format!("docx-rs parse error: {e}")))?;

    let mut lines: Vec<String> = Vec::new();
    for child in &docx.document.children {
        if let DocumentChild::Paragraph(para) = child {
            let mut para_text = String::new();
            for pchild in &para.children {
                if let ParagraphChild::Run(run) = pchild {
                    for rchild in &run.children {
                        if let RunChild::Text(t) = rchild {
                            para_text.push_str(&t.text);
                        }
                    }
                }
            }
            let trimmed = para_text.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Convert Word heading styles to Markdown headings when present
            if let Some(style) = &para.property.style {
                let val = style.val.to_ascii_lowercase();
                if val.contains("heading1") || val == "1" {
                    lines.push(format!("# {trimmed}"));
                    continue;
                } else if val.contains("heading2") || val == "2" {
                    lines.push(format!("## {trimmed}"));
                    continue;
                }
            }
            lines.push(trimmed.to_string());
        }
    }
    Ok(lines.join("\n"))
}

/// Parses a `.docx` medical book using `docx-rs` and feeds extracted paragraphs into the semantic chunker.
pub fn parse_docx_book(
    bytes: &[u8],
    defaults: &BookMetadataDefaults,
    custom_rule: Option<&CustomChunkRule>,
) -> PipelineResult<Vec<KnowledgeDocument>> {
    let joined = extract_docx_text(bytes)?;
    let lines: Vec<String> = joined.lines().map(ToString::to_string).collect();
    if joined.contains("\n# ") || joined.starts_with("# ") {
        parse_markdown_book(&joined, defaults)
    } else if custom_rule.is_some() || joined.contains("【疾病】") {
        parse_plain_text_book(&joined, defaults, custom_rule)
    } else {
        // Fallback: first line as disease name if short, rest as clinical manifestation
        let mut iter = lines.into_iter();
        let first = iter.next().unwrap_or_else(|| defaults.source_title.clone());
        let body: Vec<String> = iter.collect();
        let tree = if body.is_empty() {
            ParsedDiseaseTree {
                disease_name: defaults.source_title.clone(),
                sections: vec![("临床表现".to_string(), first)],
            }
        } else {
            ParsedDiseaseTree {
                disease_name: first,
                sections: vec![("临床表现".to_string(), body.join("\n"))],
            }
        };
        build_chunks_from_trees(vec![tree], defaults)
    }
}

/// Extracts plain text from `.pdf` bytes using `pdf-extract` + `lopdf` fallback and optional OCR.
pub fn extract_pdf_text(
    bytes: &[u8],
    ocr: Option<&dyn OcrPlugin>,
) -> PipelineResult<String> {
    // Primary text-layer extraction via pdf-extract
    let mut extracted = pdf_extract::extract_text_from_mem(bytes)
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    // Secondary text-layer extraction via lopdf
    if extracted.is_empty()
        && let Ok(doc) = lopdf::Document::load_mem(bytes)
    {
        let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
        if !pages.is_empty()
            && let Ok(text) = doc.extract_text(&pages)
        {
            extracted = text.trim().to_string();
        }
    }

    // Scanned / image-only PDF fallback -> OCR plugin
    if extracted.is_empty() {
        let plugin: &dyn OcrPlugin = ocr.unwrap_or(&NoopOcrPlugin);
        extracted = plugin.extract_text(bytes)?;
        if extracted.trim().is_empty() {
            return Err(PipelineError::ScannedPdfNeedsOcr);
        }
    }
    Ok(extracted)
}

/// Parses a `.pdf` medical book using `pdf-extract` + `lopdf` fallback, with OCR plugin hook for scanned PDFs.
pub fn parse_pdf_book(
    bytes: &[u8],
    defaults: &BookMetadataDefaults,
    custom_rule: Option<&CustomChunkRule>,
    ocr: Option<&dyn OcrPlugin>,
) -> PipelineResult<Vec<KnowledgeDocument>> {
    let extracted = extract_pdf_text(bytes, ocr)?;

    if extracted.contains("\n# ") || extracted.starts_with("# ") {
        parse_markdown_book(&extracted, defaults)
    } else if custom_rule.is_some() || extracted.contains("【疾病】") {
        parse_plain_text_book(&extracted, defaults, custom_rule)
    } else {
        let lines: Vec<&str> = extracted
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let (disease_name, body) = if lines.len() >= 2 && lines[0].chars().count() <= 40 {
            (lines[0].to_string(), lines[1..].join("\n"))
        } else {
            (defaults.source_title.clone(), lines.join("\n"))
        };
        build_chunks_from_trees(
            vec![ParsedDiseaseTree {
                disease_name,
                sections: vec![("临床表现".to_string(), body)],
            }],
            defaults,
        )
    }
}

fn build_chunks_from_trees(
    trees: Vec<ParsedDiseaseTree>,
    defaults: &BookMetadataDefaults,
) -> PipelineResult<Vec<KnowledgeDocument>> {
    let mut docs = Vec::new();
    for tree in trees {
        let disease_name = tree.disease_name.trim().to_string();
        if disease_name.is_empty() || tree.sections.is_empty() {
            continue;
        }

        // Scan sections to extract disease-level department, red flags, and seasonal tags
        let mut department = defaults.default_department.clone();
        let mut secondary_departments: Vec<String> = Vec::new();
        let mut red_flag_signs: Vec<String> = Vec::new();
        let mut seasonal_tags = defaults.seasonal_tags.clone();

        for (sec_title, sec_body) in &tree.sections {
            if sec_title.contains("推荐科室") || sec_title.contains("分诊") {
                let extracted_depts = extract_departments(sec_body);
                if let Some(primary) = extracted_depts.first() {
                    department = primary.clone();
                    secondary_departments = extracted_depts.into_iter().skip(1).collect();
                }
            }
            if sec_title.contains("急症") || sec_title.contains("红线") {
                red_flag_signs.extend(extract_phrases(sec_body));
            }
            for tag in infer_seasonal_tags(&disease_name, sec_body) {
                if !seasonal_tags.contains(&tag) {
                    seasonal_tags.push(tag);
                }
            }
        }
        red_flag_signs.sort();
        red_flag_signs.dedup();

        // Group sections so each logical knowledge chunk has clean metadata
        let mut seen_sections: HashMap<String, usize> = HashMap::new();
        for (sec_title, sec_body) in tree.sections {
            let trimmed_body = sec_body.trim();
            if trimmed_body.is_empty() {
                continue;
            }
            let section_type = map_section_type(&sec_title);
            let idx = seen_sections
                .entry(section_type.as_str().to_string())
                .and_modify(|c| *c += 1)
                .or_insert(0);

            let doc_id = deterministic_chunk_id(
                &defaults.source_title,
                &disease_name,
                section_type.as_str(),
                *idx,
            );

            let mut key_symptoms = extract_symptoms(&disease_name, trimmed_body);
            if key_symptoms.is_empty() {
                key_symptoms.push(disease_name.clone());
            }

            let target_population = infer_populations(trimmed_body, &defaults.default_population);
            let doc = KnowledgeDocument {
                doc_id,
                source_title: defaults.source_title.clone(),
                source_type: defaults.source_type,
                authority_level: defaults.authority_level.clamp(0.1, 1.0),
                disease_name: disease_name.clone(),
                aliases: Vec::new(),
                department: department.clone(),
                secondary_departments: secondary_departments.clone(),
                target_population,
                section_type,
                content: format!("[{disease_name} - {sec_title}] {trimmed_body}"),
                key_symptoms,
                differential_points: if section_type == SectionType::DifferentialDiagnosis {
                    vec![trimmed_body.to_string()]
                } else {
                    Vec::new()
                },
                red_flag_signs: if section_type == SectionType::RedFlag {
                    extract_phrases(trimmed_body)
                } else {
                    red_flag_signs.clone()
                },
                seasonal_tags: seasonal_tags.clone(),
                suggested_follow_up_slots: Vec::new(),
                updated_at: Some(Utc::now()),
            };
            doc.validate()
                .map_err(|e| PipelineError::Validation(format!("chunk {}: {e}", doc.doc_id)))?;
            docs.push(doc);
        }
    }

    Ok(docs)
}

fn map_section_type(title: &str) -> SectionType {
    if title.contains("鉴别") {
        SectionType::DifferentialDiagnosis
    } else if title.contains("急症") || title.contains("危险") || title.contains("红线") {
        SectionType::RedFlag
    } else if title.contains("推荐科室") || title.contains("分诊") {
        SectionType::TriageRule
    } else {
        SectionType::ClinicalManifestation
    }
}

fn deterministic_chunk_id(source: &str, disease: &str, section: &str, seq: usize) -> String {
    let hex_digest = hash_api_key(&format!("{source}|{disease}|{section}|{seq}"));
    format!("kb_book_{}", &hex_digest[..16])
}

fn extract_departments(text: &str) -> Vec<String> {
    const KNOWN_DEPTS: &[&str] = &[
        "急诊科",
        "呼吸内科",
        "发热门诊",
        "心血管内科",
        "消化内科",
        "神经内科",
        "内分泌科",
        "儿科",
        "妇产科",
        "骨科",
        "皮肤科",
        "耳鼻咽喉科",
        "眼科",
        "感染科",
        "肠道门诊",
        "全科医学科",
    ];
    let mut found: Vec<(usize, String)> = Vec::new();
    for dept in KNOWN_DEPTS {
        if let Some(pos) = text.find(dept) {
            found.push((pos, (*dept).to_string()));
        }
    }
    found.sort_by_key(|(pos, _)| *pos);
    found.into_iter().map(|(_, d)| d).collect()
}

fn extract_symptoms(disease_name: &str, text: &str) -> Vec<String> {
    const COMMON_SYMPTOMS: &[&str] = &[
        "高热",
        "发热",
        "发烧",
        "畏寒",
        "寒战",
        "头痛",
        "肌肉酸痛",
        "全身肌肉关节酸痛",
        "乏力",
        "咽喉痛",
        "咽痛",
        "干咳",
        "咳嗽",
        "鼻塞",
        "流涕",
        "呼吸困难",
        "胸闷",
        "胸痛",
        "大汗",
        "左臂放射痛",
        "晕厥",
        "恶心",
        "呕吐",
        "腹痛",
        "腹泻",
        "水样便",
        "脱水",
        "畏光",
        "血便",
        "意识模糊",
        "口唇发绀",
        "Fever",
        "Cough",
        "Sore Throat",
    ];
    let mut found = Vec::new();
    for sym in COMMON_SYMPTOMS {
        if text.contains(sym) && !found.iter().any(|s: &String| s == sym) {
            found.push((*sym).to_string());
        }
    }
    if found.is_empty() && !disease_name.trim().is_empty() {
        found.push(disease_name.trim().to_string());
    }
    found
}

fn extract_phrases(text: &str) -> Vec<String> {
    text.split(['、', '，', ',', '；', ';', '。', '\n'])
        .map(str::trim)
        .filter(|s| {
            let n = s.chars().count();
            (2..=24).contains(&n)
        })
        .map(ToString::to_string)
        .take(8)
        .collect()
}

fn infer_seasonal_tags(disease_name: &str, text: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let combined = format!("{disease_name} {text}");
    if combined.contains("流感") || combined.contains("Influenza") {
        tags.push("influenza".to_string());
        tags.push("winter".to_string());
    }
    if combined.contains("支原体") {
        tags.push("mycoplasma".to_string());
        tags.push("autumn_winter".to_string());
    }
    if combined.contains("胃肠炎") || combined.contains("诺如") {
        tags.push("gastroenteritis".to_string());
    }
    tags
}

fn infer_populations(text: &str, defaults: &[TargetPopulation]) -> Vec<TargetPopulation> {
    let mut pops = defaults.to_vec();
    if (text.contains("儿童") || text.contains("婴幼儿")) && !pops.contains(&TargetPopulation::Child) {
        pops.push(TargetPopulation::Child);
    }
    if (text.contains("老人") || text.contains("高龄")) && !pops.contains(&TargetPopulation::Elderly) {
        pops.push(TargetPopulation::Elderly);
    }
    if text.contains("孕妇") && !pops.contains(&TargetPopulation::Pregnant) {
        pops.push(TargetPopulation::Pregnant);
    }
    if pops.is_empty() {
        pops.push(TargetPopulation::Adult);
    }
    pops
}
