use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Category of clinical visual / multimodal media assets attached to cases or knowledge documents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClinicalMediaCategory {
    /// 伤口/创面照片（外伤、溃疡、烧烫伤、术后切口、压疮等）
    WoundImage,
    /// 核磁共振影像（MRI）
    MriImage,
    /// CT 影像
    CtImage,
    /// X 线平片影像
    XrayImage,
    /// 超声影像（B超/彩超）
    UltrasoundImage,
    /// 心电图（ECG/EKG）波形图
    EcgImage,
    /// 内镜影像（胃镜/肠镜/支气管镜等）
    EndoscopyImage,
    /// 病理切片图像
    PathologyImage,
    /// 皮肤病/皮损/皮肤镜照片
    DermatologyImage,
    /// 抽血/生化化验单拍照或扫描件
    LabReportImage,
    /// 诱因物质/过敏原实物照片（药盒、食物、植物、昆虫、化学品、接触物等）
    TriggerSubstanceImage,
    /// 药品包装/处方照片
    MedicationImage,
    /// 其他医学图像或附件
    #[default]
    Other,
}

impl ClinicalMediaCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WoundImage => "wound_image",
            Self::MriImage => "mri_image",
            Self::CtImage => "ct_image",
            Self::XrayImage => "xray_image",
            Self::UltrasoundImage => "ultrasound_image",
            Self::EcgImage => "ecg_image",
            Self::EndoscopyImage => "endoscopy_image",
            Self::PathologyImage => "pathology_image",
            Self::DermatologyImage => "dermatology_image",
            Self::LabReportImage => "lab_report_image",
            Self::TriggerSubstanceImage => "trigger_substance_image",
            Self::MedicationImage => "medication_image",
            Self::Other => "other",
        }
    }

    pub fn label_zh(&self) -> &'static str {
        match self {
            Self::WoundImage => "伤口/创面图片",
            Self::MriImage => "核磁共振(MRI)影像",
            Self::CtImage => "CT影像",
            Self::XrayImage => "X线平片",
            Self::UltrasoundImage => "超声影像",
            Self::EcgImage => "心电图(ECG)",
            Self::EndoscopyImage => "内镜影像",
            Self::PathologyImage => "病理切片图",
            Self::DermatologyImage => "皮损/皮肤镜图片",
            Self::LabReportImage => "化验单图片",
            Self::TriggerSubstanceImage => "诱因物质/过敏原图片",
            Self::MedicationImage => "药品/处方图片",
            Self::Other => "其他医学附件",
        }
    }
}

/// Multimodal medical image / media attachment (supports URI/path and/or inline base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClinicalMediaAsset {
    pub asset_id: String,
    #[serde(default)]
    pub category: ClinicalMediaCategory,
    pub title: String,
    #[serde(default = "default_image_mime")]
    pub mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri_or_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_base64: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_site: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_image_mime() -> String {
    "image/jpeg".to_string()
}

/// Structured vital signs measurement (aligned with HL7 FHIR Observation & LOINC).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VitalSignsRecord {
    #[serde(default)]
    pub record_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_at: Option<DateTime<Utc>>,
    /// 收缩压/高压 (mmHg, LOINC 8480-6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub systolic_bp_mmhg: Option<f32>,
    /// 舒张压/低压 (mmHg, LOINC 8462-4)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diastolic_bp_mmhg: Option<f32>,
    /// 心率 (次/分, LOINC 8867-4)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heart_rate_bpm: Option<f32>,
    /// 呼吸频率 (次/分, LOINC 9279-1)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respiratory_rate_rpm: Option<f32>,
    /// 体温 (℃, LOINC 8310-5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f32>,
    /// 血氧饱和度 SpO2 (%, LOINC 2708-6)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spo2_percent: Option<f32>,
    /// 指尖/随机血糖 (mmol/L, LOINC 2339-0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blood_glucose_mmol_l: Option<f32>,
    /// 身高 (cm, LOINC 8302-2)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_cm: Option<f32>,
    /// 体重 (kg, LOINC 29463-7)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_kg: Option<f32>,
    /// BMI (kg/m², LOINC 39156-5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmi: Option<f32>,
    /// NRS 疼痛评分 (0-10)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pain_score_0_10: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl VitalSignsRecord {
    pub fn has_any_measurement(&self) -> bool {
        self.systolic_bp_mmhg.is_some()
            || self.diastolic_bp_mmhg.is_some()
            || self.heart_rate_bpm.is_some()
            || self.respiratory_rate_rpm.is_some()
            || self.temperature_c.is_some()
            || self.spo2_percent.is_some()
            || self.blood_glucose_mmol_l.is_some()
            || self.height_cm.is_some()
            || self.weight_kg.is_some()
            || self.bmi.is_some()
            || self.pain_score_0_10.is_some()
    }

    pub fn summary_text(&self) -> String {
        let mut parts = Vec::new();
        if let (Some(s), Some(d)) = (self.systolic_bp_mmhg, self.diastolic_bp_mmhg) {
            parts.push(format!("血压 {s:.0}/{d:.0} mmHg"));
        } else if let Some(s) = self.systolic_bp_mmhg {
            parts.push(format!("收缩压 {s:.0} mmHg"));
        }
        if let Some(hr) = self.heart_rate_bpm {
            parts.push(format!("心率 {hr:.0} bpm"));
        }
        if let Some(t) = self.temperature_c {
            parts.push(format!("体温 {t:.1} ℃"));
        }
        if let Some(rr) = self.respiratory_rate_rpm {
            parts.push(format!("呼吸 {rr:.0} 次/分"));
        }
        if let Some(spo2) = self.spo2_percent {
            parts.push(format!("血氧 {spo2:.0}%"));
        }
        if let Some(glu) = self.blood_glucose_mmol_l {
            parts.push(format!("血糖 {glu:.1} mmol/L"));
        }
        if let Some(pain) = self.pain_score_0_10 {
            parts.push(format!("疼痛评分 {pain}/10"));
        }
        parts.join("，")
    }
}

/// Standard laboratory & biochemical panel categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LabPanelCategory {
    /// 血常规（CBC）
    CompleteBloodCount,
    /// 肝功能
    LiverFunction,
    /// 肾功能
    RenalFunction,
    /// 电解质
    Electrolytes,
    /// 血脂四项/七项
    LipidProfile,
    /// 血糖与糖化血红蛋白
    GlucoseMetabolism,
    /// 凝血功能
    Coagulation,
    /// 心肌酶与肌钙蛋白
    CardiacMarkers,
    /// 炎症指标（CRP/PCT/ESR）
    InflammationMarkers,
    /// 甲状腺功能
    ThyroidFunction,
    /// 尿常规
    Urinalysis,
    /// 过敏原特异性 IgE
    AllergenIge,
    /// 肿瘤标志物
    TumorMarkers,
    /// 动脉血气分析
    ArterialBloodGas,
    /// 病原体/血清学检查
    PathogenSerology,
    #[default]
    Other,
}

impl LabPanelCategory {
    pub fn label_zh(&self) -> &'static str {
        match self {
            Self::CompleteBloodCount => "血常规(CBC)",
            Self::LiverFunction => "肝功能生化",
            Self::RenalFunction => "肾功能生化",
            Self::Electrolytes => "电解质",
            Self::LipidProfile => "血脂分析",
            Self::GlucoseMetabolism => "糖代谢/血糖",
            Self::Coagulation => "凝血功能",
            Self::CardiacMarkers => "心肌酶与肌钙蛋白",
            Self::InflammationMarkers => "炎症指标(CRP/PCT/ESR)",
            Self::ThyroidFunction => "甲状腺功能",
            Self::Urinalysis => "尿常规",
            Self::AllergenIge => "过敏原IgE检测",
            Self::TumorMarkers => "肿瘤标志物",
            Self::ArterialBloodGas => "动脉血气分析",
            Self::PathogenSerology => "病原体/血清学",
            Self::Other => "其他检验生化",
        }
    }
}

/// Interpretation flag for a laboratory / biochemical analyte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbnormalFlag {
    Normal,
    High,
    Low,
    CriticalHigh,
    CriticalLow,
    Positive,
    Negative,
    Abnormal,
}

/// Single laboratory / biochemical analyte (e.g., WBC, ALT, Cr, cTnI, CRP, HbA1c).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabBiochemItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loinc_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_code: Option<String>,
    pub item_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_numeric: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_range: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abnormal_flag: Option<AbnormalFlag>,
}

/// Grouped laboratory / blood biochemistry report panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabBiochemPanel {
    pub panel_id: String,
    #[serde(default)]
    pub category: LabPanelCategory,
    pub panel_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specimen: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub items: Vec<LabBiochemItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default)]
    pub attachments: Vec<ClinicalMediaAsset>,
}

/// Medical imaging & functional examination modality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImagingModality {
    /// 核磁共振成像 (MRI)
    Mri,
    /// 电子计算机断层扫描 (CT)
    Ct,
    /// X 线平片 (DR/CR)
    Xray,
    /// 超声检查 (B超/彩超)
    Ultrasound,
    /// 心电图 (ECG)
    Ecg,
    /// 脑电图 (EEG)
    Eeg,
    /// 内镜检查 (胃镜/肠镜/支气管镜)
    Endoscopy,
    /// PET-CT / 核医学
    PetCt,
    /// 病理活检
    Pathology,
    /// 皮肤镜
    Dermoscopy,
    #[default]
    Other,
}

impl ImagingModality {
    pub fn label_zh(&self) -> &'static str {
        match self {
            Self::Mri => "核磁共振(MRI)",
            Self::Ct => "CT扫描",
            Self::Xray => "X线平片",
            Self::Ultrasound => "超声检查",
            Self::Ecg => "心电图(ECG)",
            Self::Eeg => "脑电图(EEG)",
            Self::Endoscopy => "内镜检查",
            Self::PetCt => "PET-CT",
            Self::Pathology => "病理检查",
            Self::Dermoscopy => "皮肤镜",
            Self::Other => "其他检查",
        }
    }
}

/// Structured imaging / radiology / functional examination record in a patient case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagingExamRecord {
    pub exam_id: String,
    #[serde(default)]
    pub modality: ImagingModality,
    pub exam_name: String,
    #[serde(default)]
    pub body_part: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examined_at: Option<DateTime<Utc>>,
    pub findings: String,
    #[serde(default)]
    pub impression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast_used: Option<bool>,
    #[serde(default)]
    pub images: Vec<ClinicalMediaAsset>,
}

/// Reference imaging feature stored in a `KnowledgeDocument` (e.g., typical MRI/CT/ECG signs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagingFeatureRef {
    #[serde(default)]
    pub modality: ImagingModality,
    pub exam_name: String,
    #[serde(default)]
    pub body_part: String,
    pub typical_findings: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_significance: Option<String>,
    #[serde(default)]
    pub reference_images: Vec<ClinicalMediaAsset>,
}

/// Wound / lesion assessment record aligned with the international TIME framework.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WoundAssessmentRecord {
    pub wound_id: String,
    pub body_site: String,
    pub wound_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_cm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_cm: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_cm: Option<f32>,
    /// T (Tissue): 基底组织类型（肉芽组织/腐肉/坏死焦痂/上皮化）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tissue_type: Option<String>,
    /// I (Infection/Inflammation): 感染/炎症征象（红肿、皮温升高、脓性分泌物、异味等）
    #[serde(default)]
    pub infection_signs: Vec<String>,
    /// M (Moisture): 渗液量与性质（无/少量/中量/大量；浆液性/血性/脓性）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exudate_level: Option<String>,
    /// E (Edge): 创缘状态（整齐/潜行/窦道/浸渍/卷边）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edge_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub images: Vec<ClinicalMediaAsset>,
}

/// Reference wound / skin lesion appearance stored in a `KnowledgeDocument`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WoundFeatureRef {
    pub wound_type: String,
    #[serde(default)]
    pub common_sites: Vec<String>,
    pub visual_characteristics: String,
    #[serde(default)]
    pub warning_signs: Vec<String>,
    #[serde(default)]
    pub reference_images: Vec<ClinicalMediaAsset>,
}

/// Category of disease trigger / allergen substance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TriggerCategory {
    /// 药物（抗生素、解热镇痛药、造影剂等）
    Drug,
    /// 食物（海鲜、坚果、芒果、牛奶、酒精等）
    Food,
    /// 化学物质（油漆、清洁剂、染发剂、农药、重金属等）
    Chemical,
    /// 植物与花粉（漆树、豚草、梧桐絮、尘螨等）
    PlantPollen,
    /// 昆虫与动物（隐翅虫、蜂毒、宠物皮屑、蛇虫咬伤等）
    InsectAnimal,
    /// 接触材料（金属镍、乳胶、化妆品、贴膏等）
    ContactMaterial,
    /// 物理与环境诱因（冷空气、紫外线、PM2.5、剧烈运动等）
    PhysicalEnvironment,
    #[default]
    Other,
}

impl TriggerCategory {
    pub fn label_zh(&self) -> &'static str {
        match self {
            Self::Drug => "药物诱因/过敏原",
            Self::Food => "食物诱因/过敏原",
            Self::Chemical => "化学物质",
            Self::PlantPollen => "植物与花粉",
            Self::InsectAnimal => "昆虫/动物毒液",
            Self::ContactMaterial => "接触物/材料",
            Self::PhysicalEnvironment => "物理与环境诱因",
            Self::Other => "其他诱因物质",
        }
    }
}

/// Severity of reaction to a trigger substance / allergen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSeverity {
    Mild,
    Moderate,
    Severe,
    Anaphylaxis,
}

/// Trigger substance / allergen record with optional visual reference photos.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggerSubstanceRecord {
    pub trigger_id: String,
    #[serde(default)]
    pub category: TriggerCategory,
    pub substance_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_route: Option<String>,
    #[serde(default)]
    pub reaction_symptoms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<TriggerSeverity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_time: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default)]
    pub images: Vec<ClinicalMediaAsset>,
}

/// Aggregated clinical examination & multimodal bundle attached to a patient visit (`L3VisitEvent`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ClinicalExamBundle {
    #[serde(default)]
    pub vital_signs: Vec<VitalSignsRecord>,
    #[serde(default)]
    pub lab_panels: Vec<LabBiochemPanel>,
    #[serde(default)]
    pub imaging_exams: Vec<ImagingExamRecord>,
    #[serde(default)]
    pub wound_assessments: Vec<WoundAssessmentRecord>,
    #[serde(default)]
    pub trigger_substances: Vec<TriggerSubstanceRecord>,
    #[serde(default)]
    pub media_assets: Vec<ClinicalMediaAsset>,
}

impl ClinicalExamBundle {
    pub fn is_empty(&self) -> bool {
        self.vital_signs.is_empty()
            && self.lab_panels.is_empty()
            && self.imaging_exams.is_empty()
            && self.wound_assessments.is_empty()
            && self.trigger_substances.is_empty()
            && self.media_assets.is_empty()
    }

    /// Merges another bundle into `self`, deduplicating by record IDs where present.
    pub fn merge(&mut self, other: ClinicalExamBundle) {
        for v in other.vital_signs {
            if v.record_id.is_empty() || !self.vital_signs.iter().any(|x| x.record_id == v.record_id)
            {
                self.vital_signs.push(v);
            }
        }
        for p in other.lab_panels {
            if let Some(existing) = self.lab_panels.iter_mut().find(|x| x.panel_id == p.panel_id) {
                *existing = p;
            } else {
                self.lab_panels.push(p);
            }
        }
        for img in other.imaging_exams {
            if let Some(existing) = self
                .imaging_exams
                .iter_mut()
                .find(|x| x.exam_id == img.exam_id)
            {
                *existing = img;
            } else {
                self.imaging_exams.push(img);
            }
        }
        for w in other.wound_assessments {
            if let Some(existing) = self
                .wound_assessments
                .iter_mut()
                .find(|x| x.wound_id == w.wound_id)
            {
                *existing = w;
            } else {
                self.wound_assessments.push(w);
            }
        }
        for t in other.trigger_substances {
            if let Some(existing) = self
                .trigger_substances
                .iter_mut()
                .find(|x| x.trigger_id == t.trigger_id)
            {
                *existing = t;
            } else {
                self.trigger_substances.push(t);
            }
        }
        for m in other.media_assets {
            if let Some(existing) = self
                .media_assets
                .iter_mut()
                .find(|x| x.asset_id == m.asset_id)
            {
                *existing = m;
            } else {
                self.media_assets.push(m);
            }
        }
    }

    /// Produces a compact clinical summary string suitable for embedding, search, and triage prompts.
    pub fn to_searchable_text(&self) -> String {
        let mut parts = Vec::new();
        for v in &self.vital_signs {
            let s = v.summary_text();
            if !s.is_empty() {
                parts.push(format!("生命体征: {s}"));
            }
        }
        for p in &self.lab_panels {
            let items: Vec<String> = p
                .items
                .iter()
                .map(|i| {
                    let val = i
                        .value_numeric
                        .map(|n| format!("{n}"))
                        .or_else(|| i.value_text.clone())
                        .unwrap_or_default();
                    let unit = i.unit.as_deref().unwrap_or("");
                    format!("{} {}{}", i.item_name, val, unit)
                })
                .collect();
            parts.push(format!("检验({}): {}", p.panel_name, items.join(", ")));
        }
        for img in &self.imaging_exams {
            parts.push(format!(
                "影像({} {}): {} {}",
                img.modality.label_zh(),
                img.body_part,
                img.findings,
                img.impression
            ));
        }
        for w in &self.wound_assessments {
            parts.push(format!(
                "伤口评估({} {}): 组织={} 感染={}",
                w.body_site,
                w.wound_type,
                w.tissue_type.as_deref().unwrap_or("未标注"),
                w.infection_signs.join("/")
            ));
        }
        for t in &self.trigger_substances {
            parts.push(format!(
                "诱因物质({} {}): {}",
                t.category.label_zh(),
                t.substance_name,
                t.reaction_symptoms.join("/")
            ));
        }
        for m in &self.media_assets {
            parts.push(format!(
                "医学图像({}): {} {}",
                m.category.label_zh(),
                m.title,
                m.caption.as_deref().unwrap_or("")
            ));
        }
        parts.join("；")
    }
}

// ===========================================================================
// Standardized Clinical Data Type Catalog (for Discovery, UI & MCP Clients)
// ===========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClinicalTypeDescriptor {
    pub code: String,
    pub name_zh: String,
    pub name_en: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StandardLabAnalyteTemplate {
    pub panel_category: LabPanelCategory,
    pub loinc_code: String,
    pub item_code: String,
    pub item_name: String,
    pub unit: String,
    pub reference_range: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClinicalDataCatalog {
    pub version: String,
    pub standards: Vec<String>,
    pub vital_sign_fields: Vec<ClinicalTypeDescriptor>,
    pub lab_panel_categories: Vec<ClinicalTypeDescriptor>,
    pub common_lab_analytes: Vec<StandardLabAnalyteTemplate>,
    pub imaging_modalities: Vec<ClinicalTypeDescriptor>,
    pub wound_time_dimensions: Vec<ClinicalTypeDescriptor>,
    pub trigger_categories: Vec<ClinicalTypeDescriptor>,
    pub media_categories: Vec<ClinicalTypeDescriptor>,
}

/// Returns the built-in standardized catalog of clinical examination, biochemical, imaging,
/// wound assessment, and trigger substance data types supported across cases and knowledge base.
pub fn default_clinical_data_catalog() -> ClinicalDataCatalog {
    let desc = |code: &str, zh: &str, en: &str, d: &str| ClinicalTypeDescriptor {
        code: code.to_string(),
        name_zh: zh.to_string(),
        name_en: en.to_string(),
        description: d.to_string(),
    };
    let analyte = |cat: LabPanelCategory, loinc: &str, code: &str, name: &str, unit: &str, r: &str| {
        StandardLabAnalyteTemplate {
            panel_category: cat,
            loinc_code: loinc.to_string(),
            item_code: code.to_string(),
            item_name: name.to_string(),
            unit: unit.to_string(),
            reference_range: r.to_string(),
        }
    };

    ClinicalDataCatalog {
        version: "2.0.0".to_string(),
        standards: vec![
            "HL7 FHIR R4 (Observation / DiagnosticReport / AllergyIntolerance / Media)".to_string(),
            "LOINC (Logical Observation Identifiers Names and Codes)".to_string(),
            "TIME Wound Assessment Framework (Tissue / Infection / Moisture / Edge)".to_string(),
        ],
        vital_sign_fields: vec![
            desc("systolic_bp_mmhg", "收缩压(高压)", "Systolic Blood Pressure", "LOINC 8480-6, 单位 mmHg, 正常参考 90-139"),
            desc("diastolic_bp_mmhg", "舒张压(低压)", "Diastolic Blood Pressure", "LOINC 8462-4, 单位 mmHg, 正常参考 60-89"),
            desc("heart_rate_bpm", "心率", "Heart Rate", "LOINC 8867-4, 单位 次/分(bpm), 正常参考 60-100"),
            desc("respiratory_rate_rpm", "呼吸频率", "Respiratory Rate", "LOINC 9279-1, 单位 次/分, 正常参考 12-20"),
            desc("temperature_c", "体温", "Body Temperature", "LOINC 8310-5, 单位 ℃, 正常参考 36.0-37.2"),
            desc("spo2_percent", "血氧饱和度(SpO2)", "Oxygen Saturation", "LOINC 2708-6, 单位 %, 正常参考 95-100"),
            desc("blood_glucose_mmol_l", "血糖", "Blood Glucose", "LOINC 2339-0, 单位 mmol/L, 空腹参考 3.9-6.1"),
            desc("bmi", "体质指数(BMI)", "Body Mass Index", "LOINC 39156-5, 单位 kg/m², 正常参考 18.5-23.9"),
            desc("pain_score_0_10", "疼痛评分(NRS)", "Pain Score", "0-10 数字疼痛评分"),
        ],
        lab_panel_categories: vec![
            desc("complete_blood_count", "血常规(CBC)", "Complete Blood Count", "白细胞、红细胞、血红蛋白、血小板、中性粒/淋巴细胞比例"),
            desc("liver_function", "肝功能生化", "Liver Function Panel", "ALT、AST、总胆红素、白蛋白、GGT、ALP"),
            desc("renal_function", "肾功能生化", "Renal Function Panel", "肌酐(Cr)、尿素氮(BUN)、尿酸(UA)、eGFR"),
            desc("electrolytes", "电解质", "Electrolytes", "钾(K+)、钠(Na+)、氯(Cl-)、钙(Ca2+)"),
            desc("lipid_profile", "血脂分析", "Lipid Profile", "总胆固醇(TC)、甘油三酯(TG)、LDL-C、HDL-C"),
            desc("glucose_metabolism", "糖代谢/血糖", "Glucose Metabolism", "空腹血糖(FPG)、餐后2h血糖、糖化血红蛋白(HbA1c)"),
            desc("coagulation", "凝血功能", "Coagulation Panel", "PT、INR、APTT、纤维蛋白原、D-二聚体"),
            desc("cardiac_markers", "心肌酶与肌钙蛋白", "Cardiac Markers", "高敏肌钙蛋白(hs-cTnI/T)、CK-MB、BNP/NT-proBNP"),
            desc("inflammation_markers", "炎症指标", "Inflammation Markers", "C反应蛋白(CRP)、降钙素原(PCT)、血沉(ESR)"),
            desc("thyroid_function", "甲状腺功能", "Thyroid Function", "TSH、FT3、FT4"),
            desc("urinalysis", "尿常规", "Urinalysis", "尿白细胞、尿蛋白、尿潜血、尿糖、酮体"),
            desc("allergen_ige", "过敏原特异性IgE", "Allergen-Specific IgE", "总IgE、吸入性及食入性过敏原筛查"),
            desc("arterial_blood_gas", "动脉血气分析", "Arterial Blood Gas", "pH、PaO2、PaCO2、乳酸(Lac)"),
            desc("tumor_markers", "肿瘤标志物", "Tumor Markers", "AFP、CEA、CA125、CA19-9、PSA"),
            desc("pathogen_serology", "病原体/血清学", "Pathogen Serology", "甲乙流抗原、支原体抗体、乙肝五项等"),
        ],
        common_lab_analytes: vec![
            analyte(LabPanelCategory::CompleteBloodCount, "6690-2", "WBC", "白细胞计数", "10^9/L", "3.5-9.5"),
            analyte(LabPanelCategory::CompleteBloodCount, "718-7", "HGB", "血红蛋白", "g/L", "115-175"),
            analyte(LabPanelCategory::CompleteBloodCount, "777-3", "PLT", "血小板计数", "10^9/L", "125-350"),
            analyte(LabPanelCategory::CompleteBloodCount, "770-8", "NEUT%", "中性粒细胞百分比", "%", "40-75"),
            analyte(LabPanelCategory::LiverFunction, "1742-6", "ALT", "丙氨酸氨基转移酶", "U/L", "7-40"),
            analyte(LabPanelCategory::LiverFunction, "1920-8", "AST", "天门冬氨酸氨基转移酶", "U/L", "13-35"),
            analyte(LabPanelCategory::LiverFunction, "1975-2", "TBIL", "总胆红素", "μmol/L", "3.4-20.5"),
            analyte(LabPanelCategory::RenalFunction, "2160-0", "Cr", "血肌酐", "μmol/L", "41-111"),
            analyte(LabPanelCategory::RenalFunction, "3094-0", "BUN", "血尿素氮", "mmol/L", "2.6-8.0"),
            analyte(LabPanelCategory::RenalFunction, "3084-1", "UA", "血尿酸", "μmol/L", "155-428"),
            analyte(LabPanelCategory::Electrolytes, "2823-3", "K+", "血钾", "mmol/L", "3.5-5.3"),
            analyte(LabPanelCategory::Electrolytes, "2951-2", "Na+", "血钠", "mmol/L", "137-147"),
            analyte(LabPanelCategory::LipidProfile, "2093-3", "TC", "总胆固醇", "mmol/L", "<5.2"),
            analyte(LabPanelCategory::LipidProfile, "2571-8", "TG", "甘油三酯", "mmol/L", "<1.7"),
            analyte(LabPanelCategory::LipidProfile, "13457-7", "LDL-C", "低密度脂蛋白胆固醇", "mmol/L", "<3.4"),
            analyte(LabPanelCategory::GlucoseMetabolism, "1558-6", "FPG", "空腹血糖", "mmol/L", "3.9-6.1"),
            analyte(LabPanelCategory::GlucoseMetabolism, "4548-4", "HbA1c", "糖化血红蛋白", "%", "4.0-6.0"),
            analyte(LabPanelCategory::Coagulation, "48065-7", "D-Dimer", "D-二聚体", "mg/L", "<0.5"),
            analyte(LabPanelCategory::CardiacMarkers, "89579-7", "hs-cTnI", "高敏心肌肌钙蛋白I", "ng/L", "<14"),
            analyte(LabPanelCategory::CardiacMarkers, "13969-1", "CK-MB", "肌酸激酶同工酶", "ng/mL", "<5.0"),
            analyte(LabPanelCategory::InflammationMarkers, "1988-5", "CRP", "C反应蛋白", "mg/L", "<10.0"),
            analyte(LabPanelCategory::InflammationMarkers, "75241-0", "PCT", "降钙素原", "ng/mL", "<0.05"),
            analyte(LabPanelCategory::ThyroidFunction, "3016-3", "TSH", "促甲状腺激素", "mIU/L", "0.27-4.20"),
            analyte(LabPanelCategory::AllergenIge, "19113-0", "Total-IgE", "血清总IgE", "IU/mL", "<100"),
        ],
        imaging_modalities: vec![
            desc("mri", "核磁共振(MRI)", "Magnetic Resonance Imaging", "软组织、颅脑、脊柱、关节、腹部高对比度成像"),
            desc("ct", "CT扫描", "Computed Tomography", "胸部、颅脑急诊出血、腹部实质脏器、血管CTA"),
            desc("xray", "X线平片(DR)", "X-Ray Radiography", "胸片、四肢骨折、腹部立卧位平片"),
            desc("ultrasound", "超声检查", "Ultrasound", "腹部肝胆脾胰肾、心脏彩超、甲状腺/乳腺、血管超声"),
            desc("ecg", "心电图(ECG)", "Electrocardiogram", "心律失常、ST-T改变、心肌缺血/梗死筛查"),
            desc("endoscopy", "内镜检查", "Endoscopy", "胃镜、肠镜、支气管镜直视与活检"),
            desc("pathology", "病理检查", "Pathology", "组织活检、细胞学检查"),
            desc("dermoscopy", "皮肤镜", "Dermoscopy", "色素性/炎症性皮损放大观察"),
        ],
        wound_time_dimensions: vec![
            desc("tissue_type", "T - 基底组织", "Tissue", "粉红肉芽组织、黄色腐肉、黑色坏死焦痂、上皮化状态"),
            desc("infection_signs", "I - 感染与炎症", "Infection / Inflammation", "创周红肿、皮温升高、压痛加剧、脓性分泌物、恶臭"),
            desc("exudate_level", "M - 渗液与湿度", "Moisture Balance", "无/少量/中量/大量；浆液性/血性/脓性渗液"),
            desc("edge_status", "E - 创缘状态", "Edge of Wound", "边缘整齐、上皮爬行、潜行/窦道、浸渍或卷边"),
        ],
        trigger_categories: vec![
            desc("drug", "药物诱因/过敏原", "Drug", "青霉素、头孢、阿司匹林、造影剂等药物及药盒图片"),
            desc("food", "食物诱因/过敏原", "Food", "海鲜、坚果、芒果、乳制品、酒精等食物图片"),
            desc("chemical", "化学物质", "Chemical", "油漆、洗洁精、染发剂、农药、强酸碱等图片"),
            desc("plant_pollen", "植物与花粉", "Plant & Pollen", "漆树、豚草、花粉、接触性致敏植物图片"),
            desc("insect_animal", "昆虫/动物毒液", "Insect & Animal", "隐翅虫、蜂类、蚊虫、毒蛇、宠物皮屑等图片"),
            desc("contact_material", "接触物/材料", "Contact Material", "金属镍、乳胶手套、贴膏、化妆品等图片"),
            desc("physical_environment", "物理与环境诱因", "Physical / Environment", "冷空气、紫外线、尘螨、雾霾、高温环境等"),
        ],
        media_categories: vec![
            desc("wound_image", "伤口/创面图片", "Wound Image", "外伤、溃疡、烧烫伤、术后切口、压疮实拍图"),
            desc("mri_image", "核磁共振(MRI)影像", "MRI Image", "T1/T2/DWI/FLAIR 等核磁共振切面图"),
            desc("ct_image", "CT影像", "CT Image", "平扫/增强CT图像"),
            desc("xray_image", "X线平片", "X-Ray Image", "DR胸片/骨科平片"),
            desc("ultrasound_image", "超声影像", "Ultrasound Image", "彩超/B超报告图"),
            desc("ecg_image", "心电图(ECG)", "ECG Image", "12导联心电图波形"),
            desc("dermatology_image", "皮损/皮肤镜图片", "Dermatology Image", "皮疹、荨麻疹、带状疱疹、湿疹等照片"),
            desc("lab_report_image", "化验单图片", "Lab Report Image", "血常规/生化化验单拍照扫描件"),
            desc("trigger_substance_image", "诱因物质/过敏原图片", "Trigger Substance Image", "引发症状或过敏的实物/包装/昆虫/植物照片"),
            desc("medication_image", "药品/处方图片", "Medication Image", "正在服用或疑似诱发不良反应的药品包装"),
        ],
    }
}
