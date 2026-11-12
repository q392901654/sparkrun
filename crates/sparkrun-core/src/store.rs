use crate::model::{GlobalSettings, Project, ProjectStore};
use std::fs;
use std::path::{Path, PathBuf};

pub fn ensure_data_dir(dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| format!("create data dir: {e}"))?;
    Ok(dir.to_path_buf())
}

fn global_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

fn projects_path(data_dir: &Path) -> PathBuf {
    data_dir.join("projects.json")
}

pub fn load_global(data_dir: &Path) -> Result<GlobalSettings, String> {
    let path = global_path(data_dir);
    if !path.exists() {
        return Ok(GlobalSettings::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read settings: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse settings: {e}"))
}

pub fn save_global(data_dir: &Path, settings: &GlobalSettings) -> Result<(), String> {
    ensure_data_dir(data_dir)?;
    let path = global_path(data_dir);
    let raw = serde_json::to_string_pretty(settings).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write settings: {e}"))
}

pub fn load_projects(data_dir: &Path) -> Result<ProjectStore, String> {
    let path = projects_path(data_dir);
    if !path.exists() {
        return Ok(ProjectStore::default());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read projects: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse projects: {e}"))
}

pub fn save_projects(data_dir: &Path, store: &ProjectStore) -> Result<(), String> {
    ensure_data_dir(data_dir)?;
    let path = projects_path(data_dir);
    let raw = serde_json::to_string_pretty(store).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write projects: {e}"))
}

/// Migrate legacy single workspace_path into a project entry.
pub fn migrate_if_needed(data_dir: &Path) -> Result<(), String> {
    let mut global = load_global(data_dir)?;
    let mut store = load_projects(data_dir)?;

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
        save_projects(data_dir, &store)?;
        save_global(data_dir, &global)?;
    }
    Ok(())
}

pub fn find_project<'a>(store: &'a ProjectStore, id: &str) -> Option<&'a Project> {
    store.projects.iter().find(|p| p.id == id)
}

pub fn find_project_mut<'a>(store: &'a mut ProjectStore, id: &str) -> Option<&'a mut Project> {
    store.projects.iter_mut().find(|p| p.id == id)
}
