use sparkrun_core::model::{GlobalSettings, Project, ProjectStore};
use sparkrun_core::store as core_store;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub(crate) fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app data dir: {e}"))?;
    core_store::ensure_data_dir(&dir)
}

pub fn load_global(app: &AppHandle) -> Result<GlobalSettings, String> {
    core_store::load_global(&data_dir(app)?)
}

pub fn save_global(app: &AppHandle, settings: &GlobalSettings) -> Result<(), String> {
    core_store::save_global(&data_dir(app)?, settings)
}

pub fn load_projects(app: &AppHandle) -> Result<ProjectStore, String> {
    core_store::load_projects(&data_dir(app)?)
}

pub fn save_projects(app: &AppHandle, store: &ProjectStore) -> Result<(), String> {
    core_store::save_projects(&data_dir(app)?, store)
}

pub fn migrate_if_needed(app: &AppHandle) -> Result<(), String> {
    core_store::migrate_if_needed(&data_dir(app)?)
}

pub fn find_project<'a>(store: &'a ProjectStore, id: &str) -> Option<&'a Project> {
    core_store::find_project(store, id)
}

pub fn find_project_mut<'a>(store: &'a mut ProjectStore, id: &str) -> Option<&'a mut Project> {
    core_store::find_project_mut(store, id)
}
