use std::collections::HashMap;

use medical_core::crypto::{EncryptedEnvelope, FieldCipher, hash_api_key, hash_patient_id};
use medical_core::models::L2SensitivePayload;
use medical_core::privacy::{
    MEDICAL_DISCLAIMER, SafetyViolation, check_medical_output_violations, sanitize_medical_output,
    scrub_pii,
};

#[test]
fn test_aes256_gcm_roundtrip_random_nonce_and_tamper_detection() {
    let cipher = FieldCipher::from_single_key([0x42_u8; 32]);
    let payload = L2SensitivePayload {
        chronic_diseases: vec!["高血压".into(), "2型糖尿病".into()],
        allergies: vec!["青霉素".into()],
        surgical_history: vec!["阑尾切除术".into()],
        long_term_medications: vec!["二甲双胍".into()],
    };

    let env1 = cipher.encrypt_json(&payload).expect("encrypt 1");
    let env2 = cipher.encrypt_json(&payload).expect("encrypt 2");

    // Fresh 96-bit random nonce every encryption -> distinct nonces & ciphertexts
    assert_ne!(env1.nonce, env2.nonce);
    assert_ne!(env1.ciphertext, env2.ciphertext);
    let raw_str = String::from_utf8_lossy(&env1.ciphertext);
    assert!(!raw_str.contains("高血压"));
    assert!(!raw_str.contains("青霉素"));

    // Compact base64 roundtrip
    let compact = env1.to_compact_base64();
    let parsed = EncryptedEnvelope::from_compact_base64(&compact).expect("parse compact");
    let decrypted: L2SensitivePayload = cipher.decrypt_json(&parsed).expect("decrypt");
    assert_eq!(decrypted, payload);

    // Tampered ciphertext must fail AEAD verification
    let mut tampered = env1.clone();
    tampered.ciphertext[0] ^= 0xFF;
    assert!(cipher.decrypt_json::<L2SensitivePayload>(&tampered).is_err());

    // Wrong key must fail decryption
    let wrong_cipher = FieldCipher::from_single_key([0x99_u8; 32]);
    assert!(wrong_cipher.decrypt_json::<L2SensitivePayload>(&env1).is_err());
}

#[test]
fn test_field_cipher_key_rotation() {
    let mut keys = HashMap::new();
    keys.insert(1, [0x11_u8; 32]);
    let cipher_v1 = FieldCipher::from_keyring(1, keys.clone()).unwrap();
    let env_v1 = cipher_v1.encrypt_bytes(b"secret-v1").unwrap();
    assert_eq!(env_v1.key_version, 1);

    keys.insert(2, [0x22_u8; 32]);
    let cipher_v2 = FieldCipher::from_keyring(2, keys).unwrap();
    let env_v2 = cipher_v2.encrypt_bytes(b"secret-v2").unwrap();
    assert_eq!(env_v2.key_version, 2);

    // v2 keyring can decrypt both v1 and v2 ciphertexts
    assert_eq!(cipher_v2.decrypt_bytes(&env_v1).unwrap(), b"secret-v1");
    assert_eq!(cipher_v2.decrypt_bytes(&env_v2).unwrap(), b"secret-v2");
}

#[test]
fn test_salted_patient_id_and_api_key_hashing() {
    let h1 = hash_patient_id("wx_openid_123", b"salt_a");
    let h2 = hash_patient_id("wx_openid_123", b"salt_a");
    let h3 = hash_patient_id("wx_openid_123", b"salt_b");

    assert!(h1.starts_with("pat_sha256_"));
    assert_eq!(h1, h2);
    assert_ne!(h1, h3);

    let kh1 = hash_api_key("mk_live_abcdef0123456789");
    let kh2 = hash_api_key("mk_live_abcdef0123456789");
    assert_eq!(kh1.len(), 64);
    assert_eq!(kh1, kh2);
}

#[test]
fn test_outbound_pii_scrubber() {
    let raw = "患者姓名：张三，身份证号110101199001011234，联系电话+86 138-0013-8000，邮箱zhangsan@example.com，家庭住址：北京市朝阳区建国路88号院3号楼。主诉发热咳嗽。";
    let res = scrub_pii(raw);

    assert!(res.was_redacted());
    assert!(!res.scrubbed_text.contains("张三"));
    assert!(!res.scrubbed_text.contains("110101199001011234"));
    assert!(!res.scrubbed_text.contains("138-0013-8000"));
    assert!(!res.scrubbed_text.contains("zhangsan@example.com"));
    assert!(!res.scrubbed_text.contains("建国路88号"));
    assert!(res.scrubbed_text.contains("[ID_REDACTED]"));
    assert!(res.scrubbed_text.contains("[PHONE_REDACTED]"));
    assert!(res.scrubbed_text.contains("[NAME_REDACTED]"));
    assert!(res.scrubbed_text.contains("[EMAIL_REDACTED]"));
    assert!(res.scrubbed_text.contains("[ADDRESS_REDACTED]"));
    assert!(res.scrubbed_text.contains("主诉发热咳嗽"));
}

#[test]
fn test_post_output_medical_safety_guardrails_and_disclaimer() {
    let unsafe_text = "根据症状您明确诊断为细菌性肺炎，建议阿莫西林每次500mg 每日三次口服。";
    let violations = check_medical_output_violations(unsafe_text);
    assert!(violations.contains(&SafetyViolation::DefinitiveDiagnosis));
    assert!(violations.contains(&SafetyViolation::PrescriptionDosage));

    let guarded = sanitize_medical_output(unsafe_text, true);
    assert!(!guarded.sanitized_text.contains("明确诊断为细菌性肺炎"));
    assert!(!guarded.sanitized_text.contains("500mg 每日三次"));
    assert!(guarded.sanitized_text.contains(MEDICAL_DISCLAIMER));
    assert!(guarded.disclaimer_appended);

    // Calling again should not duplicate disclaimer
    let guarded2 = sanitize_medical_output(&guarded.sanitized_text, true);
    assert!(!guarded2.disclaimer_appended);
    assert_eq!(
        guarded2.sanitized_text.matches(MEDICAL_DISCLAIMER).count(),
        1
    );
}

#[test]
fn test_scrub_pii_preserves_clinical_phrases_and_redacts_self_intros_and_spaced_addresses() {
    // Clinical phrases & prompt template labels must NOT be corrupted
    for phrase in [
        "患者发热咳嗽三天",
        "本人胸痛头晕",
        "本人胸闷气短两天",
        "患者本轮主诉:高热咳嗽三天，体温39.5，高热39度",
    ] {
        let res = scrub_pii(phrase);
        assert_eq!(
            res.scrubbed_text, phrase,
            "clinical phrase should be preserved intact: {phrase}"
        );
        assert!(!res.was_redacted(), "unexpected redaction on: {phrase}");
    }

    // Self-introductions and labeled names MUST be redacted
    let r_wo_jiao = scrub_pii("我叫张三，发热两天");
    assert!(!r_wo_jiao.scrubbed_text.contains("张三"));
    assert!(r_wo_jiao.scrubbed_text.contains("[NAME_REDACTED]"));
    assert!(r_wo_jiao.scrubbed_text.contains("发热两天"));

    let r_wo_shi = scrub_pii("我是张伟，头晕恶心两天");
    assert!(!r_wo_shi.scrubbed_text.contains("张伟"));
    assert!(r_wo_shi.scrubbed_text.contains("[NAME_REDACTED]"));
    assert!(r_wo_shi.scrubbed_text.contains("头晕恶心两天"));

    let r_ben_ren = scrub_pii("本人姓名：王五，咳嗽三天");
    assert!(!r_ben_ren.scrubbed_text.contains("王五"));
    assert!(r_ben_ren.scrubbed_text.contains("[NAME_REDACTED]"));
    assert!(r_ben_ren.scrubbed_text.contains("咳嗽三天"));

    let r_huan_zhe = scrub_pii("患者姓名：李四，头痛");
    assert!(!r_huan_zhe.scrubbed_text.contains("李四"));
    assert!(r_huan_zhe.scrubbed_text.contains("[NAME_REDACTED]"));
    assert!(r_huan_zhe.scrubbed_text.contains("头痛"));

    // Addresses with internal spaces must not leak street/building numbers
    let r_addr = scrub_pii("家庭住址：北京市朝阳区建国路 88 号院 3 号楼，主诉头痛");
    assert!(!r_addr.scrubbed_text.contains("88 号院"));
    assert!(!r_addr.scrubbed_text.contains("3 号楼"));
    assert!(r_addr.scrubbed_text.contains("[ADDRESS_REDACTED]"));
    assert!(r_addr.scrubbed_text.contains("主诉头痛"));
}

#[test]
fn test_sanitize_medical_output_hardened_diagnosis_and_dosage_guardrails() {
    // Definitive diagnoses (including subtle forms)
    for diag_case in [
        "初步诊断：急性胃肠炎，建议多喝水。",
        "建议多喝水。诊断：急性胃肠炎",
        "您这就是急性阑尾炎，请马上去外科。",
    ] {
        let v = check_medical_output_violations(diag_case);
        assert!(
            v.contains(&SafetyViolation::DefinitiveDiagnosis),
            "expected DefinitiveDiagnosis on: {diag_case}"
        );
        let s = sanitize_medical_output(diag_case, true);
        assert!(!s.sanitized_text.contains("急性胃肠炎"));
        assert!(!s.sanitized_text.contains("急性阑尾炎"));
        assert!(s.sanitized_text.contains(MEDICAL_DISCLAIMER));
    }

    // Prescription dosages (route-interleaved, optional-freq, prefix-freq, Chinese units/numerals)
    for (dosage_case, forbidden_fragments) in [
        (
            "阿莫西林胶囊 500mg 口服 每日三次",
            vec!["500mg", "每日", "三次"],
        ),
        ("头孢呋辛酯片 250mg 口服 bid", vec!["250mg", "bid"]),
        (
            "建议口服头孢克肟 200mg，症状不缓解再复诊。",
            vec!["200mg"],
        ),
        ("可静脉注射左氧氟沙星 0.5g。", vec!["0.5g"]),
        (
            "建议每日三次，每次服用阿莫西林 500mg。",
            vec!["每日三次", "500mg"],
        ),
        (
            "可服用布洛芬 200毫克 每日两次。",
            vec!["200毫克", "每日两次", "两次"],
        ),
        (
            "阿奇霉素干混悬剂 0.25g 每天一次",
            vec!["0.25g", "每天一次", "一次"],
        ),
        (
            "生理盐水 100毫升 静滴 每日一次",
            vec!["100毫升", "每日一次", "一次"],
        ),
        ("头孢曲松钠 1克 静滴 每日两次", vec!["1克", "每日两次", "两次"]),
    ] {
        let v = check_medical_output_violations(dosage_case);
        assert!(
            v.contains(&SafetyViolation::PrescriptionDosage),
            "expected PrescriptionDosage on: {dosage_case}"
        );
        let s = sanitize_medical_output(dosage_case, true);
        for frag in forbidden_fragments {
            assert!(
                !s.sanitized_text.contains(frag),
                "fragment '{frag}' leaked in sanitized text '{}' (input: {dosage_case})",
                s.sanitized_text
            );
        }
        assert!(s.sanitized_text.contains(MEDICAL_DISCLAIMER));
    }

    // Normal non-prescription clinical measurements must NOT false-positive
    let safe_text = "目前体温39.5，高热39度，建议多喝温水并观察2~3天。";
    assert!(check_medical_output_violations(safe_text).is_empty());
    let safe_out = sanitize_medical_output(safe_text, true);
    assert!(safe_out.violations_detected.is_empty());
    assert!(safe_out.sanitized_text.contains("体温39.5"));
    assert!(safe_out.sanitized_text.contains("高热39度"));
}

#[test]
fn test_output_guardrails_colon_diagnosis_uppercase_units_per_day_and_safe_5g_vitals() {
    // Colon-separated diagnosis, 考虑诊断, 拟诊断, and colon-preceded 诊断：
    for (diag_case, forbidden) in [
        ("确诊：急性支气管炎", "急性支气管炎"),
        ("确诊: 急性支气管炎", "急性支气管炎"),
        ("考虑诊断：急性胃肠炎", "急性胃肠炎"),
        ("拟诊断：社区获得性肺炎", "社区获得性肺炎"),
        ("医生意见：诊断：社区获得性肺炎", "社区获得性肺炎"),
    ] {
        let violations = check_medical_output_violations(diag_case);
        assert!(
            violations.contains(&SafetyViolation::DefinitiveDiagnosis),
            "expected DefinitiveDiagnosis on: {diag_case}"
        );
        let guarded = sanitize_medical_output(diag_case, true);
        assert!(
            guarded
                .violations_detected
                .contains(&SafetyViolation::DefinitiveDiagnosis)
        );
        assert!(
            !guarded.sanitized_text.contains(forbidden),
            "diagnosis '{forbidden}' leaked in '{}' (input: {diag_case})",
            guarded.sanitized_text
        );
        assert!(guarded.sanitized_text.contains(MEDICAL_DISCLAIMER));
    }

    // Uppercase/mixed-case Latin units, /天 & /日 suffixes, and 片|粒|支|袋 + route
    for (dosage_case, forbidden_frags) in [
        (
            "阿莫西林胶囊 500MG 口服 每日三次",
            vec!["500MG", "口服", "每日三次"],
        ),
        ("布洛芬 2粒/天", vec!["2粒/天", "2粒"]),
        ("1片/日", vec!["1片/日", "1片"]),
        ("头孢克肟 2片，睡前口服", vec!["2片", "睡前口服"]),
        ("2粒 口服", vec!["2粒", "口服"]),
        ("1支 肌注", vec!["1支", "肌注"]),
        ("10ML 静滴", vec!["10ML", "静滴"]),
        ("100UG 口服", vec!["100UG", "口服"]),
        ("100IU 肌注", vec!["100IU", "肌注"]),
    ] {
        let violations = check_medical_output_violations(dosage_case);
        assert!(
            violations.contains(&SafetyViolation::PrescriptionDosage),
            "expected PrescriptionDosage on: {dosage_case}"
        );
        let guarded = sanitize_medical_output(dosage_case, true);
        assert!(
            guarded
                .violations_detected
                .contains(&SafetyViolation::PrescriptionDosage)
        );
        for frag in forbidden_frags {
            assert!(
                !guarded.sanitized_text.contains(frag),
                "dosage fragment '{frag}' leaked in '{}' (input: {dosage_case})",
                guarded.sanitized_text
            );
        }
        assert!(guarded.sanitized_text.contains(MEDICAL_DISCLAIMER));
    }

    // Vital signs, 5G network, and disclaimer/non-definitive phrasing must produce ZERO violations
    for safe_case in [
        "体温39.5℃，高热39度，血氧98%",
        "5G网络",
        "尚未确诊，建议到医院进一步检查。",
    ] {
        let violations = check_medical_output_violations(safe_case);
        assert!(
            violations.is_empty(),
            "unexpected violations {violations:?} on safe text: {safe_case}"
        );
        let guarded = sanitize_medical_output(safe_case, true);
        assert!(guarded.violations_detected.is_empty());
        assert!(guarded.sanitized_text.starts_with(safe_case));
        assert!(guarded.sanitized_text.contains(MEDICAL_DISCLAIMER));
    }
}
