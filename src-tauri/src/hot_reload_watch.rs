//! Watch project sources for external editors (IDEA / Cursor).
//! Launcher's own editor save already calls `hot_reload_compile`; this covers FS changes.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotReloadFsChange {
    pub project_id: String,
    pub path: String,
}

struct WatchSession {
    project_id: String,
    stop: Arc<AtomicBool>,
}

pub struct HotReloadWatchHub {
    session: Mutex<Option<WatchSession>>,
}

impl HotReloadWatchHub {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
        }
    }

    pub fn start(&self, app: AppHandle, project_id: String, root_path: String) -> Result<(), String> {
        let root = PathBuf::from(crate::deps::normalize_path(&root_path));
        if !root.is_dir() {
            return Err(format!("watch root not a directory: {}", root.display()));
        }

        {
            let mut guard = self.session.lock().unwrap();
            if let Some(prev) = guard.as_ref() {
                if prev.project_id == project_id {
                    return Ok(());
                }
                prev.stop.store(true, Ordering::SeqCst);
            }
            *guard = None;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let project_id_bg = project_id.clone();
        let root_bg = root.clone();

        std::thread::Builder::new()
            .name("hot-reload-watch".into())
            .spawn(move || watch_loop(app, project_id_bg, root_bg, stop_bg))
            .map_err(|e| format!("spawn watch thread: {e}"))?;

        let mut guard = self.session.lock().unwrap();
        *guard = Some(WatchSession { project_id, stop });
        Ok(())
    }

    pub fn stop(&self, project_id: Option<&str>) {
        let mut guard = self.session.lock().unwrap();
        if let Some(prev) = guard.as_ref() {
            if project_id.is_none() || project_id == Some(prev.project_id.as_str()) {
                prev.stop.store(true, Ordering::SeqCst);
                *guard = None;
            }
        }
    }
}

fn watch_loop(app: AppHandle, project_id: String, root: PathBuf, stop: Arc<AtomicBool>) {
    use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = match RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            let _ = tx.send(res);
        },
        Config::default(),
    ) {
        Ok(w) => w,
        Err(e) => {
            let _ = app.emit(
                "module-log",
                crate::process_mgr::LogEvent {
                    unit_id: String::new(),
                    stream: "stderr".into(),
                    line: format!("[launcher] hot-reload watch failed to start: {e}"),
                },
            );
            return;
        }
    };

    if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
        let _ = app.emit(
            "module-log",
            crate::process_mgr::LogEvent {
                unit_id: String::new(),
                stream: "stderr".into(),
                line: format!("[launcher] hot-reload watch {}: {e}", root.display()),
            },
        );
        return;
    }

    let _ = app.emit(
        "module-log",
        crate::process_mgr::LogEvent {
            unit_id: String::new(),
            stream: "stdout".into(),
            line: format!(
                "[launcher] hot-reload: watching {} for .java/.yml/.properties (external IDE OK)",
                root.display()
            ),
        },
    );

    let mut pending: Option<(PathBuf, Instant)> = None;
    let debounce = Duration::from_millis(700);

    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(event)) => {
                if !matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
                ) {
                    continue;
                }
                for path in event.paths {
                    if is_hot_reloadable_source(&path) {
                        pending = Some((path, Instant::now()));
                    }
                }
            }
            Ok(Err(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if let Some((ref path, at)) = pending {
            if at.elapsed() >= debounce {
                let path_str = path.to_string_lossy().to_string();
                pending = None;
                let _ = app.emit(
                    "hot-reload-fs-change",
                    HotReloadFsChange {
                        project_id: project_id.clone(),
                        path: path_str,
                    },
                );
            }
        }
    }
}

fn is_hot_reloadable_source(path: &Path) -> bool {
    if path
        .components()
        .any(|c| matches!(
            c.as_os_str().to_string_lossy().as_ref(),
            "target"
                | "node_modules"
                | ".git"
                | "dist"
                | ".idea"
                | ".launcher-compile-cp.txt"
                | "project-launcher"
                | "sparkrun"
        ))
    {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    if name == ".reloadtrigger" || name.ends_with(".class") {
        return false;
    }
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "java" | "kt" | "yml" | "yaml" | "properties"
    )
}
