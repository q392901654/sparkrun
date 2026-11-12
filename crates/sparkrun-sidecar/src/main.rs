//! SparkRun NDJSON sidecar — long-lived process for VS Code / Cursor extensions.
//!
//! Protocol: one JSON object per line on stdin/stdout.
//! - Request:  `{"id":1,"method":"scan","params":{...}}`
//! - Response: `{"id":1,"ok":true,"result":...}` / `{"id":1,"ok":false,"error":"..."}`
//! - Event:    `{"event":"log","payload":{...}}` (no id)

mod protocol;
mod sink;

use protocol::{Request, Response};
use sink::NdjsonSink;
use sparkrun_core::hot_reload_watch::HotReloadWatchHub;
use sparkrun_core::job::{JobPriority, JobQueue};
use sparkrun_core::model::{BuildBeforeRun, Launchable, Project, RuntimeEnv};
use sparkrun_core::process_mgr::ProcessManager;
use sparkrun_core::scan_cache;
use sparkrun_core::scanner;
use sparkrun_core::toolchain::{detect_toolchain, discover_toolchain_homes};
use sparkrun_core::toolchain_cache;
use sparkrun_core::{EventSink as _, SharedSink};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct AppState {
    data_dir: PathBuf,
    sink: SharedSink,
    mgr: Arc<ProcessManager>,
    queue: Arc<JobQueue>,
    watch: Arc<HotReloadWatchHub>,
    out: Arc<Mutex<Box<dyn Write + Send>>>,
}

fn parse_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut data_dir: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        if a == "--data-dir" {
            if let Some(p) = args.next() {
                data_dir = Some(PathBuf::from(p));
            }
        } else if let Some(p) = a.strip_prefix("--data-dir=") {
            data_dir = Some(PathBuf::from(p));
        }
    }
    data_dir.unwrap_or_else(|| {
        let mut p = std::env::temp_dir();
        p.push("sparkrun-sidecar");
        p
    })
}

fn write_response(out: &Arc<Mutex<Box<dyn Write + Send>>>, resp: &Response) {
    if let Ok(mut g) = out.lock() {
        if let Ok(raw) = serde_json::to_string(resp) {
            let _ = writeln!(g, "{raw}");
            let _ = g.flush();
        }
    }
}

#[tokio::main]
async fn main() {
    let data_dir = parse_args();
    let _ = sparkrun_core::store::ensure_data_dir(&data_dir);

    let out: Arc<Mutex<Box<dyn Write + Send>>> =
        Arc::new(Mutex::new(Box::new(std::io::stdout())));
    let sink: SharedSink = Arc::new(NdjsonSink {
        out: Arc::clone(&out),
    });
    let mgr = Arc::new(ProcessManager::new());
    let queue = Arc::new(JobQueue::new());
    queue.set_sink(Arc::clone(&sink));
    let watch = Arc::new(HotReloadWatchHub::new());

    let state = Arc::new(AppState {
        data_dir,
        sink,
        mgr,
        queue,
        watch,
        out,
    });

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::Builder::new()
        .name("sidecar-stdin".into())
        .spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("spawn stdin thread");

    while let Some(line) = rx.recv().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                write_response(
                    &state.out,
                    &Response {
                        id: None,
                        ok: false,
                        result: None,
                        error: Some(format!("invalid request JSON: {e}")),
                    },
                );
                continue;
            }
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let id = req.id;
            let resp = match handle(Arc::clone(&state), req).await {
                Ok(result) => Response {
                    id,
                    ok: true,
                    result: Some(result),
                    error: None,
                },
                Err(e) => Response {
                    id,
                    ok: false,
                    result: None,
                    error: Some(e),
                },
            };
            write_response(&state.out, &resp);
        });
    }
}

async fn handle(state: Arc<AppState>, req: Request) -> Result<serde_json::Value, String> {
    let method = req.method.as_str();
    let params = req.params.unwrap_or(serde_json::Value::Null);
    match method {
        "ping" => Ok(serde_json::json!({"pong": true})),
        "scan" => scan(&state, params).await,
        "start" => start(&state, params).await,
        "stop" => stop(&state, params),
        "listRunning" => Ok(serde_json::to_value(state.mgr.list_running()).unwrap_or_default()),
        "detectToolchain" => detect_tc(&state, params).await,
        "discoverToolchainHomes" => discover_homes(&state, params).await,
        "termWrite" => term_write(&state, params),
        "termResize" => term_resize(&state, params),
        "build" => build(&state, params).await,
        "installLocalDeps" => install_deps(&state, params).await,
        "hotReloadCompile" => hot_reload(&state, params).await,
        "startHotReloadWatch" => start_watch(&state, params),
        "stopHotReloadWatch" => {
            let project_id = params
                .get("projectId")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            state.watch.stop(project_id.as_deref());
            Ok(serde_json::Value::Null)
        }
        "shutdown" => {
            for u in state.mgr.list_running() {
                let _ = state.mgr.stop(&state.sink, &u.unit_id);
            }
            state.watch.stop(None);
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                std::process::exit(0);
            });
            Ok(serde_json::json!({"bye": true}))
        }
        other => Err(format!("unknown method: {other}")),
    }
}

async fn scan(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let project: Project = serde_json::from_value(
        params
            .get("project")
            .cloned()
            .ok_or_else(|| "missing project".to_string())?,
    )
    .map_err(|e| format!("project: {e}"))?;
    let sink = Arc::clone(&state.sink);
    let dir = state.data_dir.clone();
    let project_id = project.id.clone();
    let root_path = project.root_path.clone();
    let title = format!("扫描 · {}", project.name);
    let units = state
        .queue
        .run(
            "scan",
            &title,
            Some(format!("scan:{project_id}")),
            JobPriority::Normal,
            true,
            move |ctx| {
                let mut last_emit = std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_millis(200))
                    .unwrap_or_else(std::time::Instant::now);
                let units = scanner::scan_project_with_progress(&project, &mut |p| {
                    ctx.set_message(p.message.clone());
                    let force = p.phase == "done"
                        || p.phase == "error"
                        || p.current == 0
                        || last_emit.elapsed().as_millis() >= 180;
                    if force {
                        last_emit = std::time::Instant::now();
                        sink.emit_scan_progress(p);
                    }
                })?;
                let _ = scan_cache::save_units_after_scan(&dir, &project_id, &root_path, &units);
                Ok(units)
            },
        )
        .await?;
    Ok(serde_json::to_value(units).map_err(|e| e.to_string())?)
}

async fn start(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let env: RuntimeEnv = serde_json::from_value(
        params
            .get("env")
            .cloned()
            .ok_or_else(|| "missing env".to_string())?,
    )
    .map_err(|e| format!("env: {e}"))?;
    let unit: Launchable = serde_json::from_value(
        params
            .get("unit")
            .cloned()
            .ok_or_else(|| "missing unit".to_string())?,
    )
    .map_err(|e| format!("unit: {e}"))?;
    let build_mode: BuildBeforeRun = params
        .get("buildBeforeRun")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("buildBeforeRun: {e}"))?
        .unwrap_or(BuildBeforeRun::Smart);
    let spring_profile = params
        .get("springProfile")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let hot_reload = params
        .get("hotReload")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mgr = Arc::clone(&state.mgr);
    let sink = Arc::clone(&state.sink);
    let title = format!("启动 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    state
        .queue
        .run(
            "start",
            &title,
            Some(key),
            JobPriority::High,
            true,
            move |ctx| {
                ctx.set_message("编译并启动…");
                mgr.start(sink, env, unit, build_mode, spring_profile, hot_reload)
            },
        )
        .await?;
    Ok(serde_json::Value::Null)
}

fn stop(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let unit_id = params
        .get("unitId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing unitId".to_string())?;
    state.mgr.stop(&state.sink, unit_id)?;
    Ok(serde_json::Value::Null)
}

async fn detect_tc(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let env: RuntimeEnv = serde_json::from_value(
        params
            .get("env")
            .cloned()
            .ok_or_else(|| "missing env".to_string())?,
    )
    .map_err(|e| format!("env: {e}"))?;
    let info = state
        .queue
        .run(
            "toolchain",
            "探测工具链",
            Some("toolchain:detect".into()),
            JobPriority::Normal,
            false,
            move |_| Ok(detect_toolchain(&env)),
        )
        .await?;
    Ok(serde_json::to_value(info).map_err(|e| e.to_string())?)
}

async fn discover_homes(
    state: &AppState,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let force = params
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let dir = state.data_dir.clone();
    if !force {
        if let Some(cached) = toolchain_cache::load_if_fresh(&dir)? {
            return Ok(serde_json::to_value(cached).map_err(|e| e.to_string())?);
        }
    }
    let homes = state
        .queue
        .run(
            "toolchain",
            "发现工具链路径",
            Some("toolchain:discover".into()),
            JobPriority::Normal,
            false,
            move |_| {
                let homes = discover_toolchain_homes();
                let _ = toolchain_cache::save(&dir, &homes);
                Ok(homes)
            },
        )
        .await?;
    Ok(serde_json::to_value(homes).map_err(|e| e.to_string())?)
}

fn term_write(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let unit_id = params
        .get("unitId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing unitId".to_string())?;
    let data = params
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing data".to_string())?;
    state.mgr.term_write(unit_id, data)?;
    Ok(serde_json::Value::Null)
}

fn term_resize(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let unit_id = params
        .get("unitId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing unitId".to_string())?;
    let cols = params.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
    state.mgr.term_resize(unit_id, cols, rows)?;
    Ok(serde_json::Value::Null)
}

async fn build(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let env: RuntimeEnv = serde_json::from_value(
        params
            .get("env")
            .cloned()
            .ok_or_else(|| "missing env".to_string())?,
    )
    .map_err(|e| format!("env: {e}"))?;
    let unit: Launchable = serde_json::from_value(
        params
            .get("unit")
            .cloned()
            .ok_or_else(|| "missing unit".to_string())?,
    )
    .map_err(|e| format!("unit: {e}"))?;
    let force = params
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mgr = Arc::clone(&state.mgr);
    let sink = Arc::clone(&state.sink);
    let title = format!("构建 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    state
        .queue
        .run(
            "compile",
            &title,
            Some(key),
            JobPriority::High,
            true,
            move |ctx| {
                ctx.set_message("编译中…");
                mgr.build_unit(sink, env, unit, force)
            },
        )
        .await?;
    Ok(serde_json::Value::Null)
}

async fn install_deps(
    state: &AppState,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let env: RuntimeEnv = serde_json::from_value(
        params
            .get("env")
            .cloned()
            .ok_or_else(|| "missing env".to_string())?,
    )
    .map_err(|e| format!("env: {e}"))?;
    let unit: Launchable = serde_json::from_value(
        params
            .get("unit")
            .cloned()
            .ok_or_else(|| "missing unit".to_string())?,
    )
    .map_err(|e| format!("unit: {e}"))?;
    let mgr = Arc::clone(&state.mgr);
    let sink = Arc::clone(&state.sink);
    let title = format!("安装依赖 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    state
        .queue
        .run(
            "compile",
            &title,
            Some(key),
            JobPriority::High,
            true,
            move |ctx| {
                ctx.set_message("mvn install…");
                mgr.install_local_deps(sink, env, unit)
            },
        )
        .await?;
    Ok(serde_json::Value::Null)
}

async fn hot_reload(
    state: &AppState,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let env: RuntimeEnv = serde_json::from_value(
        params
            .get("env")
            .cloned()
            .ok_or_else(|| "missing env".to_string())?,
    )
    .map_err(|e| format!("env: {e}"))?;
    let unit: Launchable = serde_json::from_value(
        params
            .get("unit")
            .cloned()
            .ok_or_else(|| "missing unit".to_string())?,
    )
    .map_err(|e| format!("unit: {e}"))?;
    let project_root = params
        .get("projectRoot")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing projectRoot".to_string())?
        .to_string();
    let changed_path = params
        .get("changedPath")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing changedPath".to_string())?
        .to_string();
    let restart = params
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mgr = Arc::clone(&state.mgr);
    let sink = Arc::clone(&state.sink);
    let title = format!("热更新编译 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    let msg = state
        .queue
        .run(
            "compile",
            &title,
            Some(key),
            JobPriority::High,
            true,
            move |ctx| {
                ctx.set_message("热更新编译…");
                mgr.hot_reload_compile(&sink, &env, &unit, &project_root, &changed_path, restart)
            },
        )
        .await?;
    Ok(serde_json::json!({ "message": msg }))
}

fn start_watch(state: &AppState, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let project_id = params
        .get("projectId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing projectId".to_string())?
        .to_string();
    let root_path = params
        .get("rootPath")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing rootPath".to_string())?
        .to_string();
    state
        .watch
        .start(Arc::clone(&state.sink), project_id, root_path)?;
    Ok(serde_json::Value::Null)
}
