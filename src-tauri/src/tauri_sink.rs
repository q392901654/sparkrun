use sparkrun_core::events::{
    EventSink, HotReloadFsChange, LogEvent, StatusEvent, TermDataEvent,
};
use sparkrun_core::job::JobInfo;
use sparkrun_core::model::ScanProgress;
use sparkrun_core::SharedSink;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

pub struct TauriSink(pub AppHandle);

impl EventSink for TauriSink {
    fn emit_module_log(&self, event: LogEvent) {
        let _ = self.0.emit("module-log", event);
    }
    fn emit_module_status(&self, event: StatusEvent) {
        let _ = self.0.emit("module-status", event);
    }
    fn emit_module_term(&self, event: TermDataEvent) {
        let _ = self.0.emit("module-term", event);
    }
    fn emit_scan_progress(&self, event: ScanProgress) {
        let _ = self.0.emit("scan-progress", event);
    }
    fn emit_job_updated(&self, event: JobInfo) {
        let _ = self.0.emit("job-updated", event);
    }
    fn emit_hot_reload_fs_change(&self, event: HotReloadFsChange) {
        let _ = self.0.emit("hot-reload-fs-change", event);
    }
}

pub fn sink_from_app(app: &AppHandle) -> SharedSink {
    Arc::new(TauriSink(app.clone()))
}
