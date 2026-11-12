use crate::protocol::EventMsg;
use sparkrun_core::events::{
    EventSink, HotReloadFsChange, LogEvent, StatusEvent, TermDataEvent,
};
use sparkrun_core::job::JobInfo;
use sparkrun_core::model::ScanProgress;
use std::io::Write;
use std::sync::{Arc, Mutex};

pub struct NdjsonSink {
    pub out: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl NdjsonSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let msg = EventMsg {
            event: event.to_string(),
            payload,
        };
        if let Ok(mut g) = self.out.lock() {
            if let Ok(raw) = serde_json::to_string(&msg) {
                let _ = writeln!(g, "{raw}");
                let _ = g.flush();
            }
        }
    }
}

impl EventSink for NdjsonSink {
    fn emit_module_log(&self, event: LogEvent) {
        self.emit("log", serde_json::to_value(event).unwrap_or_default());
    }
    fn emit_module_status(&self, event: StatusEvent) {
        self.emit("status", serde_json::to_value(event).unwrap_or_default());
    }
    fn emit_module_term(&self, event: TermDataEvent) {
        self.emit("term", serde_json::to_value(event).unwrap_or_default());
    }
    fn emit_scan_progress(&self, event: ScanProgress) {
        self.emit(
            "scanProgress",
            serde_json::to_value(event).unwrap_or_default(),
        );
    }
    fn emit_job_updated(&self, event: JobInfo) {
        self.emit("job", serde_json::to_value(event).unwrap_or_default());
    }
    fn emit_hot_reload_fs_change(&self, event: HotReloadFsChange) {
        self.emit(
            "hotReloadFsChange",
            serde_json::to_value(event).unwrap_or_default(),
        );
    }
}
