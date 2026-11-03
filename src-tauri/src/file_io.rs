use crate::store::{find_project, load_projects};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::fs;
use std::path::{Component, Path, PathBuf};
use tauri::AppHandle;

fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())?;
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "ico" => Some("image/x-icon"),
        _ => None,
    }
}

pub(crate) const SKIP_DIR_NAMES: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "dist",
    "build",
    ".idea",
    ".vscode",
    "__pycache__",
    ".mvn",
    "coverage",
    ".turbo",
    ".next",
];

pub(crate) fn should_skip_dir_name(name: &str) -> bool {
    SKIP_DIR_NAMES.iter().any(|s| *s == name) || (name.starts_with('.') && name != ".")
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// Resolve `path` and ensure it stays under the project's root (no escape via ..).
pub(crate) fn resolve_under_root(root: &Path, path: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("project root invalid: {e}"))?;
    let raw = PathBuf::from(path);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        root.join(raw)
    };
    let canon = candidate
        .canonicalize()
        .map_err(|e| format!("file not found: {e}"))?;
    if !canon.starts_with(&root) {
        return Err("path outside project root".into());
    }
    // Reject weird components after canonicalize (should already be clean)
    for c in canon.components() {
        if matches!(c, Component::ParentDir) {
            return Err("invalid path".into());
        }
    }
    Ok(canon)
}

pub(crate) fn project_root(app: &AppHandle, project_id: &str) -> Result<PathBuf, String> {
    let store = load_projects(app)?;
    let project = find_project(&store, project_id).ok_or_else(|| "project not found".to_string())?;
    Ok(PathBuf::from(&project.root_path))
}

#[tauri::command]
pub fn read_text_file(app: AppHandle, project_id: String, path: String) -> Result<String, String> {
    let root = project_root(&app, &project_id)?;
    let file = resolve_under_root(&root, &path)?;
    if !file.is_file() {
        return Err("not a file".into());
    }
    if image_mime(&file).is_some() {
        return Err("binary image — use read_image_data_url".into());
    }
    let meta = fs::metadata(&file).map_err(|e| format!("stat: {e}"))?;
    // Soft limit ~8 MiB to avoid freezing UI
    if meta.len() > 8 * 1024 * 1024 {
        return Err("file too large (>8MB)".into());
    }
    fs::read_to_string(&file).map_err(|e| format!("read: {e}"))
}

/// Read a common image as a `data:` URL for in-app preview (no asset protocol required).
#[tauri::command]
pub fn read_image_data_url(
    app: AppHandle,
    project_id: String,
    path: String,
) -> Result<String, String> {
    let root = project_root(&app, &project_id)?;
    let file = resolve_under_root(&root, &path)?;
    if !file.is_file() {
        return Err("not a file".into());
    }
    let mime = image_mime(&file).ok_or_else(|| "not a supported image".to_string())?;
    let meta = fs::metadata(&file).map_err(|e| format!("stat: {e}"))?;
    // Soft limit ~12 MiB for preview
    if meta.len() > 12 * 1024 * 1024 {
        return Err("image too large (>12MB)".into());
    }
    let bytes = fs::read(&file).map_err(|e| format!("read: {e}"))?;
    Ok(format!("data:{mime};base64,{}", STANDARD.encode(bytes)))
}

#[tauri::command]
pub fn write_text_file(
    app: AppHandle,
    project_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let root = project_root(&app, &project_id)?;
    // For new files, parent must exist under root; canonicalize file may fail if missing.
    let raw = PathBuf::from(&path);
    let candidate = if raw.is_absolute() {
        raw.clone()
    } else {
        root.join(&raw)
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| "invalid path".to_string())?;
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("project root invalid: {e}"))?;
    let parent_canon = parent
        .canonicalize()
        .map_err(|e| format!("parent dir: {e}"))?;
    if !parent_canon.starts_with(&root_canon) {
        return Err("path outside project root".into());
    }
    if candidate.exists() {
        let _ = resolve_under_root(&root, &path)?;
    }
    fs::write(&candidate, content.as_bytes()).map_err(|e| format!("write: {e}"))
}

/// List directory entries under project root; when `base_path` is set, path must also stay under it
/// (used for module-scoped file trees).
#[tauri::command]
pub fn list_dir(
    app: AppHandle,
    project_id: String,
    path: String,
    base_path: Option<String>,
) -> Result<Vec<DirEntryDto>, String> {
    let root = project_root(&app, &project_id)?;
    let dir = resolve_under_root(&root, &path)?;
    if !dir.is_dir() {
        return Err("not a directory".into());
    }
    if let Some(base) = base_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let base_canon = resolve_under_root(&root, base)?;
        if !dir.starts_with(&base_canon) {
            return Err("path outside module root".into());
        }
    }

    let mut entries: Vec<DirEntryDto> = Vec::new();
    let rd = fs::read_dir(&dir).map_err(|e| format!("read_dir: {e}"))?;
    for ent in rd {
        let ent = ent.map_err(|e| format!("dir entry: {e}"))?;
        let name = ent.file_name().to_string_lossy().to_string();
        if name == "." || name == ".." {
            continue;
        }
        let meta = match ent.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let is_dir = meta.is_dir();
        if is_dir && should_skip_dir_name(&name) {
            continue;
        }
        let full = ent.path();
        entries.push(DirEntryDto {
            name,
            path: full.to_string_lossy().to_string(),
            is_dir,
        });
    }
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(entries)
}
