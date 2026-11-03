//! Embedded PTY for Vue/npm — feeds an in-app xterm.js panel (not a popup console).

use crate::model::RuntimeEnv;
use crate::toolchain::build_path_env;
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermDataEvent {
    pub unit_id: String,
    /// Base64-encoded raw PTY bytes (ANSI / binary-safe).
    pub data: String,
}

pub struct PtySession {
    child: Mutex<Box<dyn PtyChild + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    pub pid: u32,
}

impl PtySession {
    pub fn spawn(
        app: AppHandle,
        unit_id: String,
        env: &RuntimeEnv,
        cwd: &Path,
        program: &Path,
        args: &[String],
        cols: u16,
        rows: u16,
    ) -> Result<Self, String> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: rows.max(8),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("open pty: {e}"))?;

        let mut cmd = CommandBuilder::new(program);
        for a in args {
            cmd.arg(a);
        }
        cmd.cwd(cwd);

        if !env.java_home.trim().is_empty() {
            cmd.env("JAVA_HOME", env.java_home.trim());
        }
        if !env.maven_home.trim().is_empty() {
            cmd.env("MAVEN_HOME", env.maven_home.trim());
            cmd.env("M2_HOME", env.maven_home.trim());
        }
        if !env.node_home.trim().is_empty() {
            cmd.env("NODE_HOME", env.node_home.trim());
        }
        cmd.env("PATH", build_path_env(env));
        cmd.env("FORCE_COLOR", "1");
        cmd.env("npm_config_color", "always");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM", "xterm-256color");
        cmd.env("npm_config_loglevel", "notice");

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("pty spawn: {e}"))?;
        let pid = child.process_id().unwrap_or(0);

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("pty reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("pty writer: {e}"))?;

        let unit_for_reader = unit_id.clone();
        let app_reader = app.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = app_reader.emit(
                            "module-term",
                            TermDataEvent {
                                unit_id: unit_for_reader.clone(),
                                data: B64.encode(&buf[..n]),
                            },
                        );
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child: Mutex::new(child),
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            pid,
        })
    }

    pub fn write(&self, data: &str) -> Result<(), String> {
        let mut w = self.writer.lock().map_err(|e| e.to_string())?;
        w.write_all(data.as_bytes())
            .map_err(|e| format!("pty write: {e}"))?;
        w.flush().map_err(|e| format!("pty flush: {e}"))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        let master = self.master.lock().map_err(|e| e.to_string())?;
        master
            .resize(PtySize {
                rows: rows.max(8),
                cols: cols.max(20),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("pty resize: {e}"))
    }

    pub fn try_wait(&self) -> Result<Option<portable_pty::ExitStatus>, String> {
        let mut child = self.child.lock().map_err(|e| e.to_string())?;
        child
            .try_wait()
            .map_err(|e| format!("pty try_wait: {e}"))
    }

    pub fn kill_tree(&self) {
        if self.pid != 0 {
            let _ = super::process_mgr::kill_process_tree(self.pid);
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}
