//! Unified job queue: priority-aware await, per-key serialization, cancel, history, UI events.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use crate::events::SharedSink;

const HISTORY_CAP: usize = 50;
const MAX_CONCURRENT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobPriority {
    High,
    Normal,
    Low,
}

fn priority_rank(p: JobPriority) -> u8 {
    match p {
        JobPriority::High => 0,
        JobPriority::Normal => 1,
        JobPriority::Low => 2,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub key: Option<String>,
    pub priority: JobPriority,
    pub status: JobStatus,
    pub cancelable: bool,
    pub message: Option<String>,
    pub error: Option<String>,
    pub created_at: u64,
    pub started_at: Option<u64>,
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobList {
    pub active: Vec<JobInfo>,
    pub history: Vec<JobInfo>,
}

#[derive(Clone)]
pub struct JobCtx {
    pub id: String,
    cancel: Arc<AtomicBool>,
    inner: Arc<JobQueueInner>,
}

impl JobCtx {
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(AtomicOrdering::SeqCst)
    }

    pub fn check_cancel(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err("已取消".into())
        } else {
            Ok(())
        }
    }

    pub fn set_message(&self, msg: impl Into<String>) {
        self.inner.set_message(&self.id, msg.into());
    }
}

struct Waiter {
    priority: JobPriority,
    seq: u64,
    tx: tokio::sync::oneshot::Sender<()>,
}

impl PartialEq for Waiter {
    fn eq(&self, other: &Self) -> bool {
        self.seq == other.seq
    }
}
impl Eq for Waiter {}
impl PartialOrd for Waiter {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Waiter {
    fn cmp(&self, other: &Self) -> Ordering {
        // max-heap: higher priority (lower rank) first, then older seq
        other
            .priority
            .rank()
            .cmp(&self.priority.rank())
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

impl JobPriority {
    fn rank(self) -> u8 {
        priority_rank(self)
    }
}

struct JobQueueInner {
    sink: Mutex<Option<SharedSink>>,
    next_id: AtomicU64,
    next_seq: AtomicU64,
    active: Mutex<HashMap<String, JobInfo>>,
    history: Mutex<VecDeque<JobInfo>>,
    cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    key_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Available concurrency slots (0..=MAX_CONCURRENT).
    slots: Mutex<usize>,
    waiters: Mutex<BinaryHeap<Waiter>>,
}

pub struct JobQueue {
    inner: Arc<JobQueueInner>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl JobQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(JobQueueInner {
                sink: Mutex::new(None),
                next_id: AtomicU64::new(1),
                next_seq: AtomicU64::new(1),
                active: Mutex::new(HashMap::new()),
                history: Mutex::new(VecDeque::new()),
                cancel_flags: Mutex::new(HashMap::new()),
                key_locks: Mutex::new(HashMap::new()),
                slots: Mutex::new(MAX_CONCURRENT),
                waiters: Mutex::new(BinaryHeap::new()),
            }),
        }
    }

    pub fn set_sink(&self, sink: SharedSink) {
        *self.inner.sink.lock().unwrap() = Some(sink);
    }

    fn emit(&self, info: &JobInfo) {
        if let Some(sink) = self.inner.sink.lock().unwrap().as_ref() {
            sink.emit_job_updated(info.clone());
        }
    }

    fn key_lock(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = self.inner.key_locks.lock().unwrap();
        map.entry(key.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub fn list(&self) -> JobList {
        let active: Vec<JobInfo> = {
            let guard = self.inner.active.lock().unwrap();
            let mut v: Vec<_> = guard.values().cloned().collect();
            v.sort_by(|a, b| {
                priority_rank(a.priority)
                    .cmp(&priority_rank(b.priority))
                    .then(a.created_at.cmp(&b.created_at))
            });
            v
        };
        let history: Vec<JobInfo> = self.inner.history.lock().unwrap().iter().cloned().collect();
        JobList { active, history }
    }

    pub fn cancel(&self, job_id: &str) -> Result<(), String> {
        if let Some(flag) = self.inner.cancel_flags.lock().unwrap().get(job_id) {
            flag.store(true, AtomicOrdering::SeqCst);
        }
        let mut active = self.inner.active.lock().unwrap();
        if let Some(info) = active.get_mut(job_id) {
            if matches!(info.status, JobStatus::Queued) {
                info.status = JobStatus::Cancelled;
                info.finished_at = Some(now_ms());
                info.message = Some("已取消".into());
                let done = info.clone();
                active.remove(job_id);
                drop(active);
                self.inner.cancel_flags.lock().unwrap().remove(job_id);
                self.push_history(done.clone());
                self.emit(&done);
                return Ok(());
            }
            if matches!(info.status, JobStatus::Running) && info.cancelable {
                info.message = Some("正在取消…".into());
                let snap = info.clone();
                drop(active);
                self.emit(&snap);
                return Ok(());
            }
            return Err("任务无法取消".into());
        }
        Err("任务不存在".into())
    }

    pub fn clear_history(&self) {
        self.inner.history.lock().unwrap().clear();
    }

    fn push_history(&self, info: JobInfo) {
        let mut h = self.inner.history.lock().unwrap();
        h.push_front(info);
        while h.len() > HISTORY_CAP {
            h.pop_back();
        }
    }

    fn finish(&self, id: &str, status: JobStatus, error: Option<String>) {
        let mut active = self.inner.active.lock().unwrap();
        if let Some(mut info) = active.remove(id) {
            info.status = status.clone();
            info.error = error;
            info.finished_at = Some(now_ms());
            if info.message.is_none() {
                info.message = match status {
                    JobStatus::Succeeded => Some("完成".into()),
                    JobStatus::Failed => Some("失败".into()),
                    JobStatus::Cancelled => Some("已取消".into()),
                    _ => None,
                };
            }
            drop(active);
            self.inner.cancel_flags.lock().unwrap().remove(id);
            self.emit(&info);
            self.push_history(info);
        }
    }

    /// Acquire a global concurrency slot, preferring higher priority waiters.
    async fn acquire_slot(&self, priority: JobPriority) -> Result<(), String> {
        {
            let mut slots = self.inner.slots.lock().unwrap();
            if *slots > 0 {
                *slots -= 1;
                return Ok(());
            }
        }
        let seq = self.inner.next_seq.fetch_add(1, AtomicOrdering::SeqCst);
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner.waiters.lock().unwrap().push(Waiter {
            priority,
            seq,
            tx,
        });
        // In case a slot freed between check and push:
        self.try_dispatch();
        rx.await.map_err(|_| "任务排队中断".to_string())
    }

    fn release_slot(&self) {
        {
            let mut slots = self.inner.slots.lock().unwrap();
            *slots += 1;
        }
        self.try_dispatch();
    }

    fn try_dispatch(&self) {
        loop {
            let mut slots = self.inner.slots.lock().unwrap();
            if *slots == 0 {
                return;
            }
            let mut waiters = self.inner.waiters.lock().unwrap();
            let Some(w) = waiters.pop() else {
                return;
            };
            *slots -= 1;
            drop(slots);
            drop(waiters);
            if w.tx.send(()).is_err() {
                // waiter gone — reclaim slot and continue
                let mut slots = self.inner.slots.lock().unwrap();
                *slots += 1;
                continue;
            }
            return;
        }
    }

    /// Submit a blocking job and await its result. Optional `key` serializes same-key jobs.
    pub async fn run<T, F>(
        &self,
        kind: &str,
        title: &str,
        key: Option<String>,
        priority: JobPriority,
        cancelable: bool,
        f: F,
    ) -> Result<T, String>
    where
        F: FnOnce(JobCtx) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        let id = format!("j{}", self.inner.next_id.fetch_add(1, AtomicOrdering::SeqCst));
        let cancel = Arc::new(AtomicBool::new(false));
        let info = JobInfo {
            id: id.clone(),
            kind: kind.to_string(),
            title: title.to_string(),
            key: key.clone(),
            priority,
            status: JobStatus::Queued,
            cancelable,
            message: None,
            error: None,
            created_at: now_ms(),
            started_at: None,
            finished_at: None,
        };

        self.inner
            .cancel_flags
            .lock()
            .unwrap()
            .insert(id.clone(), cancel.clone());
        self.inner
            .active
            .lock()
            .unwrap()
            .insert(id.clone(), info.clone());
        self.emit(&info);

        let key_guard = if let Some(ref k) = key {
            Some(self.key_lock(k).lock_owned().await)
        } else {
            None
        };

        if cancel.load(AtomicOrdering::SeqCst) {
            self.finish(&id, JobStatus::Cancelled, None);
            return Err("已取消".into());
        }

        self.acquire_slot(priority).await?;

        if cancel.load(AtomicOrdering::SeqCst) {
            self.release_slot();
            self.finish(&id, JobStatus::Cancelled, None);
            return Err("已取消".into());
        }

        {
            let mut active = self.inner.active.lock().unwrap();
            if let Some(j) = active.get_mut(&id) {
                j.status = JobStatus::Running;
                j.started_at = Some(now_ms());
                let snap = j.clone();
                drop(active);
                self.emit(&snap);
            }
        }

        let ctx = JobCtx {
            id: id.clone(),
            cancel: cancel.clone(),
            inner: self.inner.clone(),
        };

        let result = tokio::task::spawn_blocking(move || f(ctx))
            .await
            .map_err(|e| format!("任务 join 失败: {e}"))?;

        self.release_slot();
        drop(key_guard);

        match &result {
            Ok(_) => {
                if cancel.load(AtomicOrdering::SeqCst) {
                    self.finish(&id, JobStatus::Cancelled, None);
                    Err("已取消".into())
                } else {
                    self.finish(&id, JobStatus::Succeeded, None);
                    result
                }
            }
            Err(e) => {
                if cancel.load(AtomicOrdering::SeqCst) || e == "已取消" {
                    self.finish(&id, JobStatus::Cancelled, None);
                    Err("已取消".into())
                } else {
                    self.finish(&id, JobStatus::Failed, Some(e.clone()));
                    result
                }
            }
        }
    }
}

impl JobQueueInner {
    fn set_message(&self, id: &str, msg: String) {
        let mut active = self.active.lock().unwrap();
        if let Some(info) = active.get_mut(id) {
            info.message = Some(msg);
            let snap = info.clone();
            drop(active);
            if let Some(sink) = self.sink.lock().unwrap().as_ref() {
                sink.emit_job_updated(snap);
            }
        }
    }
}

