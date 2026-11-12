#!/usr/bin/env python3
"""One-shot: rewrite copied core modules to use EventSink instead of AppHandle."""
from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / "crates" / "sparkrun-core" / "src"


def replace_emits(text: str, sink_name: str = "sink") -> str:
    text = re.sub(
        rf"let _ = {sink_name}\.emit\(\s*\"module-status\",\s*(StatusEvent\s*\{{.*?\}})\s*,?\s*\);",
        rf"{sink_name}.emit_module_status(\1);",
        text,
        flags=re.S,
    )
    text = re.sub(
        rf"let _ = {sink_name}\.emit\(\s*\"module-log\",\s*(LogEvent\s*\{{.*?\}})\s*,?\s*\);",
        rf"{sink_name}.emit_module_log(\1);",
        text,
        flags=re.S,
    )
    text = re.sub(
        rf"let _ = {sink_name}\.emit\(\s*\"module-term\",\s*(TermDataEvent\s*\{{.*?\}})\s*,?\s*\);",
        rf"{sink_name}.emit_module_term(\1);",
        text,
        flags=re.S,
    )
    text = re.sub(
        rf"let _ = {sink_name}\.emit\(\s*\"hot-reload-fs-change\",\s*(HotReloadFsChange\s*\{{.*?\}})\s*,?\s*\);",
        rf"{sink_name}.emit_hot_reload_fs_change(\1);",
        text,
        flags=re.S,
    )
    text = re.sub(
        rf"let _ = {sink_name}\.emit\(\s*\"job-updated\",\s*([^)]+)\);",
        rf"{sink_name}.emit_job_updated(\1);",
        text,
        flags=re.S,
    )
    text = re.sub(
        rf"let _ = {sink_name}\.emit\(\s*\"scan-progress\",\s*([^)]+)\);",
        rf"{sink_name}.emit_scan_progress(\1);",
        text,
        flags=re.S,
    )
    return text


def rewrite_process_mgr() -> None:
    p = ROOT / "process_mgr.rs"
    t = p.read_text(encoding="utf-8")
    t = re.sub(
        r"#\[derive\(Debug, Clone, Serialize, Deserialize\)\]\n"
        r"#\[serde\(rename_all = \"camelCase\"\)\]\n"
        r"pub struct LogEvent \{[^}]+\}\n\n",
        "",
        t,
    )
    t = re.sub(
        r"#\[derive\(Debug, Clone, Serialize, Deserialize\)\]\n"
        r"#\[serde\(rename_all = \"camelCase\"\)\]\n"
        r"pub struct StatusEvent \{[^}]+\}\n\n",
        "",
        t,
    )
    t = t.replace(
        "use tauri::{AppHandle, Emitter};",
        "use crate::events::{LogEvent, SharedSink, StatusEvent};",
    )
    t = t.replace("app: AppHandle,", "sink: SharedSink,")
    t = t.replace("app: &AppHandle,", "sink: &SharedSink,")

    # Rename remaining AppHandle param names app -> sink in signatures already done;
    # rewrite body identifiers carefully.
    out_lines = []
    for line in t.splitlines(True):
        if "apply_" in line or "Application" in line or "apple" in line.lower():
            out_lines.append(line)
            continue
        line = re.sub(r"\bAppHandle\b", "SharedSink", line)
        line = re.sub(r"\bapp\b", "sink", line)
        out_lines.append(line)
    t = "".join(out_lines)

    t = t.replace("sink.clone()", "Arc::clone(&sink)")
    t = replace_emits(t, "sink")
    # nested clones used in threads
    for name in ("sink2", "sink_out", "sink_err", "sink_reader"):
        t = replace_emits(t, name)

    # fix common renamed locals from app2 etc. after \bapp\b -> sink
    # app2 became sink2 already if word was app2? No — app2 doesn't match \bapp\b
    t = t.replace("let app2 = Arc::clone(&sink);", "let sink2 = Arc::clone(&sink);")
    t = t.replace("let app_out = Arc::clone(&sink);", "let sink_out = Arc::clone(&sink);")
    t = t.replace("let app_err = Arc::clone(&sink);", "let sink_err = Arc::clone(&sink);")
    t = t.replace("app2.", "sink2.")
    t = t.replace("app_out.", "sink_out.")
    t = t.replace("app_err.", "sink_err.")

    for name in ("sink2", "sink_out", "sink_err"):
        t = replace_emits(t, name)

    remaining = [f"{i}:{l}" for i, l in enumerate(t.splitlines(), 1) if ".emit(" in l]
    if remaining:
        print("process_mgr remaining emits:")
        print("\n".join(remaining[:40]))
    p.write_text(t, encoding="utf-8")
    print("wrote process_mgr.rs")


def rewrite_pty() -> None:
    p = ROOT / "pty_session.rs"
    t = p.read_text(encoding="utf-8")
    t = re.sub(
        r"#\[derive\(Debug, Clone, Serialize, Deserialize\)\]\n"
        r"#\[serde\(rename_all = \"camelCase\"\)\]\n"
        r"pub struct TermDataEvent \{[^}]+\}\n\n",
        "",
        t,
    )
    t = t.replace(
        "use tauri::{AppHandle, Emitter};",
        "use crate::events::{SharedSink, TermDataEvent};",
    )
    # drop unused serde if TermDataEvent removed — keep base64
    t = t.replace("use serde::{Deserialize, Serialize};\n", "")
    t = t.replace("app: AppHandle,", "sink: SharedSink,")
    out = []
    for line in t.splitlines(True):
        line = re.sub(r"\bapp\b", "sink", line)
        out.append(line)
    t = "".join(out)
    t = t.replace("let sink_reader = sink.clone();", "let sink_reader = Arc::clone(&sink);")
    t = t.replace("let sink_for_reader = sink_id", "LET_SKIP")
    # original: let app_reader = app.clone();
    t = t.replace("let sink_reader = Arc::clone(&sink);", "let sink_reader = Arc::clone(&sink);")
    if "sink.clone()" in t:
        t = t.replace("sink.clone()", "Arc::clone(&sink)")
    t = replace_emits(t, "sink")
    t = replace_emits(t, "sink_reader")
    # Fix reader thread variable names from rewrite
    t = t.replace("let unit_for_reader = unit_id.clone();\n        let sink_reader = Arc::clone(&sink);",
                  "let unit_for_reader = unit_id.clone();\n        let sink_reader = Arc::clone(&sink);")
    # After \bapp\b, app_reader became sink_reader only if it was exactly app — app_reader stays
    t = t.replace("let app_reader = Arc::clone(&sink);", "let sink_reader = Arc::clone(&sink);")
    t = t.replace("app_reader.", "sink_reader.")
    t = replace_emits(t, "sink_reader")
    if "use std::sync::Mutex;" in t and "use std::sync::Arc;" not in t:
        t = t.replace("use std::sync::Mutex;", "use std::sync::{Arc, Mutex};")
    remaining = [f"{i}:{l}" for i, l in enumerate(t.splitlines(), 1) if ".emit(" in l]
    if remaining:
        print("pty remaining emits:", remaining)
    p.write_text(t, encoding="utf-8")
    print("wrote pty_session.rs")


def rewrite_job() -> None:
    p = ROOT / "job.rs"
    t = p.read_text(encoding="utf-8")
    t = t.replace(
        "use tauri::{AppHandle, Emitter, State};",
        "use crate::events::SharedSink;",
    )
    t = t.replace("app: Mutex<Option<AppHandle>>,", "sink: Mutex<Option<SharedSink>>,")
    t = t.replace(
        "app: Mutex::new(None),",
        "sink: Mutex::new(None),",
    )
    t = t.replace(
        """    pub fn set_app(&self, app: AppHandle) {
        *self.inner.app.lock().unwrap() = Some(app);
    }

    fn emit(&self, info: &JobInfo) {
        if let Some(app) = self.inner.app.lock().unwrap().as_ref() {
            let _ = app.emit("job-updated", info.clone());
        }
    }""",
        """    pub fn set_sink(&self, sink: SharedSink) {
        *self.inner.sink.lock().unwrap() = Some(sink);
    }

    fn emit(&self, info: &JobInfo) {
        if let Some(sink) = self.inner.sink.lock().unwrap().as_ref() {
            sink.emit_job_updated(info.clone());
        }
    }""",
    )
    t = t.replace(
        "tauri::async_runtime::spawn_blocking(move || f(ctx))",
        "tokio::task::spawn_blocking(move || f(ctx))",
    )
    t = t.replace(
        """            if let Some(app) = self.app.lock().unwrap().as_ref() {
                let _ = app.emit("job-updated", snap);
            }""",
        """            if let Some(sink) = self.sink.lock().unwrap().as_ref() {
                sink.emit_job_updated(snap);
            }""",
    )
    # Remove tauri command wrappers — stay in tauri crate
    t = re.sub(
        r"\n#\[tauri::command\][\s\S]*$",
        "\n",
        t,
    )
    p.write_text(t, encoding="utf-8")
    print("wrote job.rs")


def rewrite_hot_reload() -> None:
    p = ROOT / "hot_reload_watch.rs"
    t = p.read_text(encoding="utf-8")
    t = t.replace(
        "use serde::Serialize;\n",
        "use crate::events::{HotReloadFsChange, LogEvent, SharedSink};\n",
    )
    t = t.replace("use tauri::{AppHandle, Emitter};\n", "")
    t = re.sub(
        r"#\[derive\(Clone, Serialize\)\]\n"
        r"#\[serde\(rename_all = \"camelCase\"\)\]\n"
        r"pub struct HotReloadFsChange \{[^}]+\}\n\n",
        "",
        t,
    )
    t = t.replace(
        "pub fn start(&self, app: AppHandle, project_id: String, root_path: String)",
        "pub fn start(&self, sink: SharedSink, project_id: String, root_path: String)",
    )
    t = t.replace(
        "fn watch_loop(app: AppHandle, project_id: String, root: PathBuf, stop: Arc<AtomicBool>)",
        "fn watch_loop(sink: SharedSink, project_id: String, root: PathBuf, stop: Arc<AtomicBool>)",
    )
    out = []
    for line in t.splitlines(True):
        line = re.sub(r"\bapp\b", "sink", line)
        out.append(line)
    t = "".join(out)
    # LogEvent path was crate::process_mgr::LogEvent
    t = t.replace("crate::process_mgr::LogEvent", "LogEvent")
    t = replace_emits(t, "sink")
    remaining = [f"{i}:{l}" for i, l in enumerate(t.splitlines(), 1) if ".emit(" in l]
    if remaining:
        print("hot_reload remaining:", remaining)
    p.write_text(t, encoding="utf-8")
    print("wrote hot_reload_watch.rs")


def rewrite_store() -> None:
    p = ROOT / "store.rs"
    p.write_text(
        '''use crate::model::{GlobalSettings, Project, ProjectStore};
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
''',
        encoding="utf-8",
    )
    print("wrote store.rs")


def rewrite_scan_cache() -> None:
    p = ROOT / "scan_cache.rs"
    t = p.read_text(encoding="utf-8")
    t = t.replace("use crate::store::data_dir;\n", "")
    t = t.replace("use tauri::AppHandle;\n", "use std::path::Path;\n")
    t = t.replace(
        """fn cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = data_dir(app)?.join("scan-cache");
    fs::create_dir_all(&dir).map_err(|e| format!("create scan-cache dir: {e}"))?;
    Ok(dir)
}

fn cache_path(app: &AppHandle, project_id: &str) -> Result<PathBuf, String> {""",
        """fn cache_dir(data_dir: &Path) -> Result<PathBuf, String> {
    let dir = data_dir.join("scan-cache");
    fs::create_dir_all(&dir).map_err(|e| format!("create scan-cache dir: {e}"))?;
    Ok(dir)
}

fn cache_path(data_dir: &Path, project_id: &str) -> Result<PathBuf, String> {""",
    )
    t = t.replace("Ok(cache_dir(app)?.join(format!(\"{safe}.json\")))",
                  "Ok(cache_dir(data_dir)?.join(format!(\"{safe}.json\")))")
    t = t.replace("app: &AppHandle", "data_dir: &Path")
    t = t.replace("cache_path(app,", "cache_path(data_dir,")
    t = t.replace("load_scan_cache(app,", "load_scan_cache(data_dir,")
    t = t.replace("save_scan_cache(app,", "save_scan_cache(data_dir,")
    p.write_text(t, encoding="utf-8")
    print("wrote scan_cache.rs")


def rewrite_toolchain_cache() -> None:
    p = ROOT / "toolchain_cache.rs"
    t = p.read_text(encoding="utf-8")
    t = t.replace("use crate::store::data_dir;\n", "")
    t = t.replace("use tauri::AppHandle;\n", "use std::path::Path;\n")
    t = t.replace(
        """fn cache_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("toolchain-homes-cache.json"))
}""",
        """fn cache_path(data_dir: &Path) -> Result<PathBuf, String> {
    Ok(data_dir.join("toolchain-homes-cache.json"))
}""",
    )
    t = t.replace("app: &AppHandle", "data_dir: &Path")
    t = t.replace("cache_path(app)?", "cache_path(data_dir)?")
    p.write_text(t, encoding="utf-8")
    print("wrote toolchain_cache.rs")


def fix_events_module_status() -> None:
    # Move ModuleStatus into events to break cycle — patch events to use local enum
    # Instead: keep StatusEvent.status as string? No — keep ModuleStatus in process_mgr
    # and define StatusEvent in process_mgr, EventSink methods take generics via serde_json
    # Simpler fix: put ModuleStatus in events.rs
    pm = (ROOT / "process_mgr.rs").read_text(encoding="utf-8")
    m = re.search(
        r"(#\[derive\(Debug, Clone, Serialize, Deserialize, PartialEq, Eq\)\]\n"
        r"#\[serde\(rename_all = \"camelCase\"\)\]\n"
        r"pub enum ModuleStatus \{[^}]+\}\n)",
        pm,
    )
    if not m:
        raise SystemExit("ModuleStatus not found")
    enum_src = m.group(1)
    pm = pm.replace(enum_src + "\n", "")
    # re-export from events
    if "use crate::events::{LogEvent, SharedSink, StatusEvent};" in pm:
        pm = pm.replace(
            "use crate::events::{LogEvent, SharedSink, StatusEvent};",
            "use crate::events::{LogEvent, ModuleStatus, SharedSink, StatusEvent};",
        )
    (ROOT / "process_mgr.rs").write_text(pm, encoding="utf-8")

    ev = (ROOT / "events.rs").read_text(encoding="utf-8")
    ev = ev.replace(
        "pub status: crate::process_mgr::ModuleStatus,",
        "pub status: ModuleStatus,",
    )
    if "pub enum ModuleStatus" not in ev:
        insert = enum_src + "\n"
        ev = ev.replace(
            "pub type SharedSink = Arc<dyn EventSink>;\n",
            "pub type SharedSink = Arc<dyn EventSink>;\n\n" + insert,
        )
    (ROOT / "events.rs").write_text(ev, encoding="utf-8")
    print("moved ModuleStatus to events")


def main() -> None:
    rewrite_store()
    rewrite_scan_cache()
    rewrite_toolchain_cache()
    rewrite_process_mgr()
    rewrite_pty()
    rewrite_job()
    rewrite_hot_reload()
    fix_events_module_status()


if __name__ == "__main__":
    main()
