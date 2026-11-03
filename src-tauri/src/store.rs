use crate::model::{GlobalSettings, Project, ProjectStore};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub(crate) fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app data dir: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(dir)
}

fn global_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("settings.json"))
}

fn projects_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("projects.json"))
}

pub fn load_global(app: &AppHandle) -> Result<GlobalSettings, String> {
    let path = global_path(app)?;
    if !path.exists() {
        return Ok(GlobalSettings::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read settings: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse settings: {e}"))
}

pub fn save_global(app: &AppHandle, settings: &GlobalSettings) -> Result<(), String> {
    let path = global_path(app)?;
    let raw = serde_json::to_string_pretty(settings).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write settings: {e}"))
}

pub fn load_projects(app: &AppHandle) -> Result<ProjectStore, String> {
    let path = projects_path(app)?;
    if !path.exists() {
        return Ok(ProjectStore::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read projects: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse projects: {e}"))
}

pub fn save_projects(app: &AppHandle, store: &ProjectStore) -> Result<(), String> {
    let path = projects_path(app)?;
    let raw = serde_json::to_string_pretty(store).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write projects: {e}"))
}

/// Migrate legacy single workspace_path into a project entry.
pub fn migrate_if_needed(app: &AppHandle) -> Result<(), String> {
    let mut global = load_global(app)?;
    let mut store = load_projects(app)?;

    if store.projects.is_empty() && !global.workspace_path.trim().is_empty() {
        let root = global.workspace_path.trim().to_string();
        let name = std::path::Path::new(&root)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "默认项目".to_string());
        let project = Project::new(name, root);
        global.active_project_id = Some(project.id.clone());
        global.workspace_path.clear();
        store.projects.push(project);
        save_projects(app, &store)?;
        save_global(app, &global)?;
    }
    Ok(())
}

pub fn find_project<'a>(store: &'a ProjectStore, id: &str) -> Option<&'a Project> {
    store.projects.iter().find(|p| p.id == id)
}

pub fn find_project_mut<'a>(store: &'a mut ProjectStore, id: &str) -> Option<&'a mut Project> {
    store.projects.iter_mut().find(|p| p.id == id)
}
