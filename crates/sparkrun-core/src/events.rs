//! Host-agnostic event sink (Tauri emit / sidecar NDJSON / tests).

use crate::job::JobInfo;
use crate::model::ScanProgress;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type SharedSink = Arc<dyn EventSink>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ModuleStatus {
    Idle,
    Starting,
    Running,
    Stopped,
    Failed,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEvent {
    pub unit_id: String,
    pub stream: String,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusEvent {
    pub unit_id: String,
    pub status: ModuleStatus,
    pub pid: Option<u32>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermDataEvent {
    pub unit_id: String,
    /// Base64-encoded raw PTY bytes (ANSI / binary-safe).
    pub data: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HotReloadFsChange {
    pub project_id: String,
    pub path: String,
}

pub trait EventSink: Send + Sync {
    fn emit_module_log(&self, event: LogEvent);
    fn emit_module_status(&self, event: StatusEvent);
    fn emit_module_term(&self, event: TermDataEvent);
    fn emit_scan_progress(&self, event: ScanProgress);
    fn emit_job_updated(&self, event: JobInfo);
    fn emit_hot_reload_fs_change(&self, event: HotReloadFsChange);
}

/// No-op sink for tests / headless one-shots.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit_module_log(&self, _event: LogEvent) {}
    fn emit_module_status(&self, _event: StatusEvent) {}
    fn emit_module_term(&self, _event: TermDataEvent) {}
    fn emit_scan_progress(&self, _event: ScanProgress) {}
    fn emit_job_updated(&self, _event: JobInfo) {}
    fn emit_hot_reload_fs_change(&self, _event: HotReloadFsChange) {}
}

pub fn null_sink() -> SharedSink {
    Arc::new(NullSink)
}
