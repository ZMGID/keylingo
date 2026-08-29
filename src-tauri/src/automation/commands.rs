use tauri::AppHandle;

use super::storage;
use super::types::{Automation, AutomationMeta};

#[tauri::command]
pub fn automation_list(app: AppHandle) -> Result<Vec<AutomationMeta>, String> {
    storage::list(&app)
}

#[tauri::command]
pub fn automation_get(app: AppHandle, id: String) -> Result<Automation, String> {
    storage::get(&app, &id)
}

#[tauri::command]
pub fn automation_save(app: AppHandle, automation: Automation) -> Result<Automation, String> {
    storage::save(&app, automation)
}

#[tauri::command]
pub fn automation_delete(app: AppHandle, id: String) -> Result<(), String> {
    storage::delete(&app, &id)
}

#[tauri::command]
pub fn automation_set_enabled(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<Automation, String> {
    storage::set_enabled(&app, &id, enabled)
}
