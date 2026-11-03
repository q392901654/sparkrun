use crate::model::{Launchable, ScanCache};
use crate::store::data_dir;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

fn cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = data_dir(app)?.join("scan-cache");
    fs::create_dir_all(&dir).map_err(|e| format!("create scan-cache dir: {e}"))?;
    Ok(dir)
}

fn cache_path(app: &AppHandle, project_id: &str) -> Result<PathBuf, String> {
    // Sanitize id for filesystem
    let safe: String = project_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Ok(cache_dir(app)?.join(format!("{safe}.json")))
}

pub fn load_scan_cache(app: &AppHandle, project_id: &str) -> Result<Option<ScanCache>, String> {
    let path = cache_path(app, project_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read scan cache: {e}"))?;
    let cache: ScanCache =
        serde_json::from_str(&raw).map_err(|e| format!("parse scan cache: {e}"))?;
    Ok(Some(cache))
}

pub fn save_scan_cache(app: &AppHandle, cache: &ScanCache) -> Result<(), String> {
    let path = cache_path(app, &cache.project_id)?;
    let raw = serde_json::to_string_pretty(cache).map_err(|e| format!("serialize scan cache: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write scan cache: {e}"))
}

pub fn clear_scan_cache(app: &AppHandle, project_id: &str) -> Result<(), String> {
    let path = cache_path(app, project_id)?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("remove scan cache: {e}"))?;
    }
    Ok(())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// After a fresh scan: write units + timestamp; keep selection fields from previous cache.
pub fn save_units_after_scan(
    app: &AppHandle,
    project_id: &str,
    root_path: &str,
    units: &[Launchable],
) -> Result<(), String> {
    let prev = load_scan_cache(app, project_id)?.unwrap_or(ScanCache {
        project_id: project_id.to_string(),
        root_path: root_path.to_string(),
        scanned_at: 0,
        units: Vec::new(),
        active_unit_id: None,
        profile_by_unit: HashMap::new(),
        node_home_by_unit: HashMap::new(),
        starred_unit_ids: Vec::new(),
        starred_files_by_unit: HashMap::new(),
    });

    let mut profile_by_unit = prev.profile_by_unit;
    for u in units {
        if !u.selected_spring_profile.is_empty() {
            profile_by_unit
                .entry(u.id.clone())
                .or_insert_with(|| u.selected_spring_profile.clone());
        }
    }

    // Drop overrides for units that no longer exist after rescan.
    let unit_ids: HashSet<&str> = units.iter().map(|u| u.id.as_str()).collect();
    let node_home_by_unit: HashMap<String, String> = prev
        .node_home_by_unit
        .into_iter()
        .filter(|(id, path)| unit_ids.contains(id.as_str()) && !path.trim().is_empty())
        .collect();

    let starred_unit_ids: Vec<String> = prev
        .starred_unit_ids
        .into_iter()
        .filter(|id| unit_ids.contains(id.as_str()))
        .collect();

    let starred_files_by_unit: HashMap<String, Vec<String>> = prev
        .starred_files_by_unit
        .into_iter()
        .filter(|(id, _)| unit_ids.contains(id.as_str()))
        .map(|(id, paths)| {
            let mut seen = HashSet::new();
            let cleaned: Vec<String> = paths
                .into_iter()
                .filter(|p| {
                    let t = p.trim();
                    !t.is_empty() && seen.insert(t.to_string())
                })
                .collect();
            (id, cleaned)
        })
        .filter(|(_, paths)| !paths.is_empty())
        .collect();

    let active_unit_id = prev
        .active_unit_id
        .filter(|id| units.iter().any(|u| &u.id == id))
        .or_else(|| units.first().map(|u| u.id.clone()));

    let cache = ScanCache {
        project_id: project_id.to_string(),
        root_path: root_path.to_string(),
        scanned_at: now_ms(),
        units: units.to_vec(),
        active_unit_id,
        profile_by_unit,
        node_home_by_unit,
        starred_unit_ids,
        starred_files_by_unit,
    };
    save_scan_cache(app, &cache)
}
