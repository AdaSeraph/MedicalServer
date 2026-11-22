pub mod ai_normalize;
pub mod commands;
pub mod state;

pub use state::StudioState;

pub fn crate_name() -> &'static str {
    "medical-studio"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let runtime = tokio::runtime::Runtime::new().expect("create tokio runtime for MedicalStudio");
    let studio_state = runtime
        .block_on(StudioState::new_offline())
        .expect("initialize MedicalStudio offline state");

    tauri::Builder::default()
        .manage(studio_state)
        .invoke_handler(tauri::generate_handler![
            // Module 1: Knowledge Import Workshop
            commands::preview_knowledge_chunks,
            commands::start_knowledge_import,
            commands::resume_knowledge_import,
            commands::get_import_job_status,
            commands::export_import_errors,
            commands::get_ai_import_prompts,
            commands::ai_normalize_knowledge_batch,
            commands::batch_preview_knowledge_items,
            commands::commit_knowledge_documents,
            commands::get_knowledge_categories,
            commands::list_knowledge_documents,
            commands::ai_inspect_knowledge_batch,
            // Module 2: Patient Import & Masking Center
            commands::preview_patient_import,
            commands::apply_masking_and_import_patients,
            commands::ai_normalize_patient_batch,
            commands::batch_preview_patient_items,
            commands::commit_patient_records,
            commands::ai_inspect_patient_batch,
            // Module 3: Seasonal & Epidemic Context Console
            commands::list_epidemic_contexts,
            commands::upsert_epidemic_context,
            commands::delete_epidemic_context,
            commands::notify_server_hot_reload,
            // Module 4: Keyword & Red-Flag Dictionary Manager
            commands::get_medical_dictionaries,
            commands::save_symptom_synonym,
            commands::delete_symptom_synonym,
            commands::save_red_flag_rule,
            commands::delete_red_flag_rule,
            commands::test_dictionary_sandbox,
            // Module 5: ECS Triage Whitebox Debugger
            commands::run_ecs_whitebox_triage,
            commands::reset_ecs_debug_session,
            commands::list_debug_patients,
            commands::get_ecs_runtime_status,
            // Module 6: AI LLM & Open API Gateway Config Center
            commands::list_llm_providers,
            commands::upsert_llm_provider,
            commands::delete_llm_provider,
            commands::test_llm_provider_connectivity,
            commands::get_router_config,
            commands::save_router_config,
            commands::list_api_keys,
            commands::issue_api_key,
            commands::revoke_api_key,
            commands::get_gateway_metrics,
            // Module 7: Data Export & Peer Exchange
            commands::export_exchange_package,
            commands::preview_exchange_package,
            commands::import_exchange_package,
            // Module 8: Patient Revisit Feedback & New Patient Consultation
            commands::lookup_patient_by_serial,
            commands::list_patient_summaries,
            commands::create_or_update_patient_profile,
            commands::submit_patient_revisit_feedback,
            commands::run_consultation_triage,
            // Module 9: Clinical Governance, Audit Logs & Safe Deletion
            commands::list_triage_audit_logs,
            commands::delete_knowledge_documents,
            commands::delete_patient_record,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MedicalStudio Tauri application");
}
