//! MedicalStudio crate (M4 implementation pending).
pub fn crate_name() -> &'static str {
    "medical-studio"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _context: tauri::Context<tauri::Wry> = tauri::generate_context!();
}
