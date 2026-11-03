//! Disk cache for `discover_toolchain_homes` (env / PATH / known folders).
//! Avoids repeating the same probe on every "查找" / idle warm within TTL.

use crate::store::data_dir;
use crate::toolchain::DiscoverToolchainHomes;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::AppHandle;

/// Freshness window for toolchain home discovery (~24h).
pub const TTL_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolchainHomesCacheFile {
    saved_at: u64,
    homes: DiscoverToolchainHomes,
}

fn cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("toolchain-homes-cache.json"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Return cached homes if file exists and is within TTL.
pub fn load_if_fresh(app: &AppHandle) -> Result<Option<DiscoverToolchainHomes>, String> {
    let path = cache_path(app)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read toolchain homes cache: {e}"))?;
    let file: ToolchainHomesCacheFile =
        serde_json::from_str(&raw).map_err(|e| format!("parse toolchain homes cache: {e}"))?;
    let age = now_ms().saturating_sub(file.saved_at);
    if age > TTL_MS {
        return Ok(None);
    }
    Ok(Some(file.homes))
}

pub fn save(app: &AppHandle, homes: &DiscoverToolchainHomes) -> Result<(), String> {
    let path = cache_path(app)?;
    let file = ToolchainHomesCacheFile {
        saved_at: now_ms(),
        homes: homes.clone(),
    };
    let raw =
        serde_json::to_string_pretty(&file).map_err(|e| format!("serialize toolchain homes cache: {e}"))?;
    fs::write(&path, raw).map_err(|e| format!("write toolchain homes cache: {e}"))
}
