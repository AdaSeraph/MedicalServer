use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

pub const MEDICAL_DISCLAIMER: &str =
    "本建议由 AI 智能导诊结合医学知识库生成，仅供参考，不替代执业医师诊断。";

// PII patterns
static RE_ID_18_EXACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b\d{6}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dx]\b")
        .expect("valid id18")
});
static RE_ID_15_EXACT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b\d{6}\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}\b")
        .expect("valid id15")
});
static RE_CN_ID_EMBEDDED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:1[1-5]|2[1-3]|3[1-7]|4[1-6]|5[0-4]|6[1-5]|71|81|82)\d{4}(?:19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dx]")
        .expect("valid cn id18")
});
static RE_CN_MOBILE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\+?86[-\s]?)?1[3-9]\d[-\s]?\d{4}[-\s]?\d{4}")
        .expect("valid cn mobile")
});
static RE_CN_LANDLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"0\d{2,3}[-\s]\d{7,8}")
        .expect("valid landline")
});
static RE_LABELED_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<label>(?:患者姓名|本人姓名|真实姓名|姓名|病人|名字|联系人|家属|接诊医师|接诊医生|主治医生|主治医师|管床医生|责任护士|经治医师|经治医生|签字医生|手术医师)\s*[:：]\s*)[\u{4e00}-\u{9fa5}·]{2,6}")
        .expect("valid labeled name")
});
static RE_MEDICAL_RECORD_NO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<label>(?:住院号|病案号|门诊号|医保卡号|社保卡号|就诊卡号|登记号)\s*[:：]?\s*)[A-Za-z0-9\-]{5,24}")
        .expect("valid medical record number")
});
static RE_BED_NO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<label>床号\s*[:：]?\s*)[A-Za-z0-9\-]{1,8}床?")
        .expect("valid bed number")
});
static RE_WECHAT_QQ: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?P<label>(?:微信号|微信|wechat|qq号|qq)\s*[:：]\s*)[A-Za-z0-9_\-]{5,32}")
        .expect("valid wechat or qq")
});
static RE_DOB_LABELED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<label>(?:出生日期|出生年月|生日)\s*[:：]\s*)(?:19|20)\d{2}[-/年\.](?:0?[1-9]|1[0-2])(?:[-/月\.](?:0?[1-9]|[12]\d|3[01])日?)?")
        .expect("valid dob")
});
static RE_SELF_INTRO_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<prefix>我叫|名叫|本人叫|我的名字(?:是|叫)|名字(?:是|叫)|姓名(?:是|为)|我是)\s*[\u{4e00}-\u{9fa5}·]{2,4}")
        .expect("valid self intro name")
});
static RE_LABELED_ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<label>(?:家庭住址|详细地址|住址|地址|居住地)\s*[:：]\s*)[^\r\n,，。；;]{4,80}")
        .expect("valid labeled address")
});
static RE_STREET_ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\u{4e00}-\u{9fa5}]{2,8}(?:省|市|区|县)[\u{4e00}-\u{9fa5}0-9A-Za-z\-]{2,30}(?:路|街|道|巷|村|小区|栋|单元|室)(?:[0-9A-Za-z\-\u{4e00}-\u{9fa5}]|\s+\d+(?:\s*(?:号院|号楼|号|栋|单元|室|楼))?){0,16}")
        .expect("valid street address")
});
static RE_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}")
        .expect("valid email")
});

// Output safety guardrail patterns
static RE_DEFINITIVE_DIAGNOSIS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?P<sep>^|[，,。；;：:\s])诊断[:：]\s*[^\n。；;！!，,]{1,24}|(?:明确诊断为|已确诊为|初步诊断[:：为是]?|临床诊断[:：为是]?|考虑诊断[:：为是]?|拟诊断[:：为是]?|确诊(?:[:：为是]|\s+)|可以诊断为|诊断为|诊断结果[:：为是]|您这就是|您患有|属于典型的)[^\n。；;！!，,]{1,24}",
    )
    .expect("valid diagnosis guardrail")
});
static RE_DOSAGE_FREQUENCY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?:",
        r"(?:每日|每天|一日)\s*[一二两三四五六\d]+\s*次",
        r"(?:[，,\s]*(?:每次|一次)\s*(?:(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服)\s*)?(?:[\u{4e00}-\u{9fa5}A-Za-z0-9]{1,16}\s*)?)?",
        r"|(?:每次|一次)\s*(?:(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服)\s*)?(?:[\u{4e00}-\u{9fa5}A-Za-z0-9]{1,16}\s*)?",
        r")\d+(?:\.\d+)?\s*(?:(?:(?i:mg|ml|ug|μg|iu)|g)\b|毫克|微克|毫升|克|单位|片|粒|支|袋)",
        r"(?:\s*[,，/／\s]*(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服))?",
        r"(?:\s*[,，/／\s]*(?:(?:每日|每天|一日)\s*[一二两三四五六\d]+\s*(?:次|片|粒|支|袋)?|(?i:bid|tid|qid|qd|q8h|q12h|qn|prn)\b|[一二两三四五六\d]+\s*次(?:\s*[/／]\s*(?:天|日))?)|\s*[/／]\s*(?:天|日))?",
        r"(?:\s*[,，/／\s]*(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服))?",
    ))
    .expect("valid dosage frequency")
});
static RE_PRESCRIPTION_DOSAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?:(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服)\s*(?:[\u{4e00}-\u{9fa5}A-Za-z0-9]{1,16}\s*)?)?",
        r"\d+(?:\.\d+)?\s*(?:(?:(?i:mg|ml|ug|μg|iu)|g)\b|毫克|微克|毫升|克|单位)",
        r"(?:\s*[,，/／\s]*(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服))?",
        r"(?:\s*[,，/／\s]*(?:(?:每日|每天|一日)\s*[一二两三四五六\d]+\s*(?:次|片|粒|支|袋)?|(?i:bid|tid|qid|qd|q8h|q12h|qn|prn)\b|[一二两三四五六\d]+\s*次(?:\s*[/／]\s*(?:天|日))?)|\s*[/／]\s*(?:天|日))?",
        r"(?:\s*[,，/／\s]*(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服))?",
        r"|(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服)\s*(?:[\u{4e00}-\u{9fa5}A-Za-z0-9]{1,16}\s*)?",
        r"\d+(?:\.\d+)?\s*(?:片|粒|支|袋)",
        r"(?:\s*[,，/／\s]*(?:(?:每日|每天|一日)\s*[一二两三四五六\d]+\s*(?:次|片|粒|支|袋)?|(?i:bid|tid|qid|qd|q8h|q12h|qn|prn)\b|[一二两三四五六\d]+\s*次(?:\s*[/／]\s*(?:天|日))?)|\s*[/／]\s*(?:天|日))?",
        r"|\d+(?:\.\d+)?\s*(?:片|粒|支|袋)",
        r"(?:",
        r"\s*[,，、/／\s]*(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服)",
        r"(?:\s*[,，/／\s]*(?:(?:每日|每天|一日)\s*[一二两三四五六\d]+\s*(?:次|片|粒|支|袋)?|(?i:bid|tid|qid|qd|q8h|q12h|qn|prn)\b|[一二两三四五六\d]+\s*次(?:\s*[/／]\s*(?:天|日))?)|\s*[/／]\s*(?:天|日))?",
        r"|",
        r"(?:\s*[,，/／\s]*(?:(?:每日|每天|一日)\s*[一二两三四五六\d]+\s*(?:次|片|粒|支|袋)?|(?i:bid|tid|qid|qd|q8h|q12h|qn|prn)\b|[一二两三四五六\d]+\s*次(?:\s*[/／]\s*(?:天|日))?)|\s*[/／]\s*(?:天|日))",
        r"(?:\s*[,，、/／\s]*(?:(?:睡前|饭前|饭后|餐前|餐后|空腹|晨起)\s*)?(?:口服|静滴|静脉注射|肌注|外涂|外用|含服|皮下注射|服用|冲服|送服|顿服))?",
        r")",
    ))
    .expect("valid dosage guardrail")
});

static RE_UNSAFE_MED_DIRECTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?:建议|可以|请|不妨)?(?:立即|马上|直接|先|自行)?",
        r"(?:停用|停服|停止服用|自行购买|直接购买|去药店买|自行服用|加量服用|减量服用)",
        r"[\u{4e00}-\u{9fa5}A-Za-z0-9]{1,18}",
        r"(?:服用|治疗|观察|即可|处理)?"
    ))
    .expect("valid unsafe med directive")
});

fn is_contraindication_context(text: &str, match_start: usize) -> bool {
    let prefix = &text[..match_start];
    let window: String = prefix
        .chars()
        .rev()
        .take(18)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    const CONTRA_MARKERS: &[&str] = &[
        "禁用", "忌用", "禁止", "切勿", "不可", "不得", "严禁", "避免", "勿用", "不要", "过敏",
    ];
    CONTRA_MARKERS.iter().any(|m| window.contains(m))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrubResult {
    pub scrubbed_text: String,
    pub redactions: Vec<String>,
}

impl ScrubResult {
    pub fn was_redacted(&self) -> bool {
        !self.redactions.is_empty()
    }
}

pub fn scrub_pii(text: &str) -> ScrubResult {
    let mut redactions = Vec::new();
    let mut out = text.to_string();

    if RE_CN_ID_EMBEDDED.is_match(&out) {
        out = RE_CN_ID_EMBEDDED
            .replace_all(&out, "[ID_REDACTED]")
            .into_owned();
        redactions.push("id_card".to_string());
    }
    if RE_ID_18_EXACT.is_match(&out) {
        out = RE_ID_18_EXACT
            .replace_all(&out, "[ID_REDACTED]")
            .into_owned();
        if !redactions.contains(&"id_card".to_string()) {
            redactions.push("id_card".to_string());
        }
    }
    if RE_ID_15_EXACT.is_match(&out) {
        out = RE_ID_15_EXACT
            .replace_all(&out, "[ID_REDACTED]")
            .into_owned();
        if !redactions.contains(&"id_card".to_string()) {
            redactions.push("id_card".to_string());
        }
    }

    if RE_CN_MOBILE.is_match(&out) {
        out = RE_CN_MOBILE
            .replace_all(&out, "[PHONE_REDACTED]")
            .into_owned();
        redactions.push("phone".to_string());
    }
    if RE_CN_LANDLINE.is_match(&out) {
        out = RE_CN_LANDLINE
            .replace_all(&out, "[PHONE_REDACTED]")
            .into_owned();
        if !redactions.contains(&"phone".to_string()) {
            redactions.push("phone".to_string());
        }
    }

    if RE_EMAIL.is_match(&out) {
        out = RE_EMAIL
            .replace_all(&out, "[EMAIL_REDACTED]")
            .into_owned();
        redactions.push("email".to_string());
    }

    if RE_LABELED_NAME.is_match(&out) {
        out = RE_LABELED_NAME
            .replace_all(&out, "${label}[NAME_REDACTED]")
            .into_owned();
        redactions.push("name".to_string());
    }
    if RE_SELF_INTRO_NAME.is_match(&out) {
        out = RE_SELF_INTRO_NAME
            .replace_all(&out, "${prefix}[NAME_REDACTED]")
            .into_owned();
        if !redactions.contains(&"name".to_string()) {
            redactions.push("name".to_string());
        }
    }

    if RE_MEDICAL_RECORD_NO.is_match(&out) {
        out = RE_MEDICAL_RECORD_NO
            .replace_all(&out, "${label}[RECORD_ID_REDACTED]")
            .into_owned();
        redactions.push("medical_record_no".to_string());
    }
    if RE_BED_NO.is_match(&out) {
        out = RE_BED_NO
            .replace_all(&out, "${label}[BED_REDACTED]")
            .into_owned();
        redactions.push("bed_no".to_string());
    }
    if RE_WECHAT_QQ.is_match(&out) {
        out = RE_WECHAT_QQ
            .replace_all(&out, "${label}[CONTACT_REDACTED]")
            .into_owned();
        redactions.push("social_contact".to_string());
    }
    if RE_DOB_LABELED.is_match(&out) {
        out = RE_DOB_LABELED
            .replace_all(&out, "${label}[DOB_REDACTED]")
            .into_owned();
        redactions.push("dob".to_string());
    }

    if RE_LABELED_ADDRESS.is_match(&out) {
        out = RE_LABELED_ADDRESS
            .replace_all(&out, "${label}[ADDRESS_REDACTED]")
            .into_owned();
        redactions.push("address".to_string());
    }
    if RE_STREET_ADDRESS.is_match(&out) {
        out = RE_STREET_ADDRESS
            .replace_all(&out, "[ADDRESS_REDACTED]")
            .into_owned();
        if !redactions.contains(&"address".to_string()) {
            redactions.push("address".to_string());
        }
    }

    ScrubResult {
        scrubbed_text: out,
        redactions,
    }
}

/// Returns `true` if the text contains high-risk unredacted PII (ID card, mobile/landline phone,
/// email, or hospital medical record number) that MUST NOT enter persistent clinical storage.
pub fn contains_high_risk_pii(text: &str) -> bool {
    RE_CN_ID_EMBEDDED.is_match(text)
        || RE_ID_18_EXACT.is_match(text)
        || RE_ID_15_EXACT.is_match(text)
        || RE_CN_MOBILE.is_match(text)
        || RE_CN_LANDLINE.is_match(text)
        || RE_EMAIL.is_match(text)
        || RE_MEDICAL_RECORD_NO.is_match(text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyViolation {
    DefinitiveDiagnosis,
    PrescriptionDosage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardrailOutput {
    pub sanitized_text: String,
    pub violations_detected: Vec<SafetyViolation>,
    pub disclaimer_appended: bool,
}

fn replace_non_contraindicated(re: &Regex, text: &str, replacement: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut last_end = 0usize;
    let mut replaced = false;
    for m in re.find_iter(text) {
        if is_contraindication_context(text, m.start()) {
            continue;
        }
        out.push_str(&text[last_end..m.start()]);
        out.push_str(replacement);
        last_end = m.end();
        replaced = true;
    }
    if !replaced {
        return (text.to_string(), false);
    }
    out.push_str(&text[last_end..]);
    (out, true)
}

fn has_non_contraindicated_match(re: &Regex, text: &str) -> bool {
    re.find_iter(text)
        .any(|m| !is_contraindication_context(text, m.start()))
}

pub fn check_medical_output_violations(text: &str) -> Vec<SafetyViolation> {
    let mut violations = Vec::new();
    if RE_DEFINITIVE_DIAGNOSIS.is_match(text) {
        violations.push(SafetyViolation::DefinitiveDiagnosis);
    }
    if has_non_contraindicated_match(&RE_PRESCRIPTION_DOSAGE, text)
        || has_non_contraindicated_match(&RE_DOSAGE_FREQUENCY, text)
        || has_non_contraindicated_match(&RE_UNSAFE_MED_DIRECTIVE, text)
    {
        violations.push(SafetyViolation::PrescriptionDosage);
    }
    violations
}

pub fn sanitize_medical_output(text: &str, append_disclaimer: bool) -> GuardrailOutput {
    let mut violations_detected = Vec::new();
    let mut out = text.to_string();

    if RE_DEFINITIVE_DIAGNOSIS.is_match(&out) {
        violations_detected.push(SafetyViolation::DefinitiveDiagnosis);
        out = RE_DEFINITIVE_DIAGNOSIS
            .replace_all(&out, "${sep}提示存在相关临床倾向（具体需由执业医师面诊评估）")
            .into_owned();
    }

    let mut dosage_violated = false;
    let (next_out, hit1) = replace_non_contraindicated(
        &RE_DOSAGE_FREQUENCY,
        &out,
        "（具体用药及剂量请严格遵从执业医师处方）",
    );
    out = next_out;
    dosage_violated |= hit1;

    let (next_out, hit2) = replace_non_contraindicated(
        &RE_PRESCRIPTION_DOSAGE,
        &out,
        "（具体用药及剂量请严格遵从执业医师处方）",
    );
    out = next_out;
    dosage_violated |= hit2;

    let (next_out, hit3) = replace_non_contraindicated(
        &RE_UNSAFE_MED_DIRECTIVE,
        &out,
        "（切勿自行停药或购药，具体用药调整请严格遵从执业医师指导）",
    );
    out = next_out;
    dosage_violated |= hit3;

    if dosage_violated {
        violations_detected.push(SafetyViolation::PrescriptionDosage);
    }

    let mut disclaimer_appended = false;
    if append_disclaimer && !out.contains(MEDICAL_DISCLAIMER) {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(MEDICAL_DISCLAIMER);
        disclaimer_appended = true;
    }

    GuardrailOutput {
        sanitized_text: out,
        violations_detected,
        disclaimer_appended,
    }
}
