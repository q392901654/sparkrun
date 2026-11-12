mod file_io;
mod search;
mod store;
mod tauri_sink;

use sparkrun_core::hot_reload_watch::HotReloadWatchHub;
use sparkrun_core::job::{JobList, JobPriority, JobQueue};
use sparkrun_core::model::{
    GlobalSettings, Launchable, Project, RuntimeEnv, ScanCache, ScanProgress,
};
use sparkrun_core::process_mgr::{ModuleRuntime, ProcessManager};
use sparkrun_core::scan_cache;
use sparkrun_core::scanner;
use sparkrun_core::toolchain::{
    detect_toolchain, discover_toolchain_homes, DiscoverToolchainHomes, ToolchainInfo,
};
use sparkrun_core::toolchain_cache;
use sparkrun_core::EventSink as _;
use std::sync::Arc;
use store::{
    data_dir, find_project, find_project_mut, load_global, load_projects, migrate_if_needed,
    save_global, save_projects,
};
use tauri::{Manager, State};
use tauri_sink::sink_from_app;

#[tauri::command]
fn get_global_settings(app: tauri::AppHandle) -> Result<GlobalSettings, String> {
    migrate_if_needed(&app)?;
    load_global(&app)
}

#[tauri::command]
fn save_global_settings(app: tauri::AppHandle, settings: GlobalSettings) -> Result<(), String> {
    save_global(&app, &settings)
}

#[tauri::command]
fn list_projects(app: tauri::AppHandle) -> Result<Vec<Project>, String> {
    migrate_if_needed(&app)?;
    Ok(load_projects(&app)?.projects)
}

#[tauri::command]
fn create_project(
    app: tauri::AppHandle,
    name: String,
    root_path: String,
) -> Result<Project, String> {
    let mut store = load_projects(&app)?;
    let project = Project::new(name, root_path);
    let mut global = load_global(&app)?;
    global.active_project_id = Some(project.id.clone());
    store.projects.push(project.clone());
    save_projects(&app, &store)?;
    save_global(&app, &global)?;
    Ok(project)
}

#[tauri::command]
fn update_project(app: tauri::AppHandle, project: Project) -> Result<Project, String> {
    let mut store = load_projects(&app)?;
    let id = project.id.clone();
    let slot = find_project_mut(&mut store, &id).ok_or_else(|| "project not found".to_string())?;
    *slot = project.clone();
    save_projects(&app, &store)?;
    Ok(project)
}

#[tauri::command]
fn delete_project(app: tauri::AppHandle, project_id: String) -> Result<(), String> {
    let mut store = load_projects(&app)?;
    store.projects.retain(|p| p.id != project_id);
    let mut global = load_global(&app)?;
    if global.active_project_id.as_deref() == Some(project_id.as_str()) {
        global.active_project_id = store.projects.first().map(|p| p.id.clone());
    }
    save_projects(&app, &store)?;
    save_global(&app, &global)?;
    let _ = scan_cache::clear_scan_cache(&data_dir(&app)?, &project_id);
    Ok(())
}

#[tauri::command]
fn set_active_project(app: tauri::AppHandle, project_id: Option<String>) -> Result<(), String> {
    let mut global = load_global(&app)?;
    global.active_project_id = project_id;
    save_global(&app, &global)
}

#[tauri::command]
async fn detect_toolchain_cmd(
    app: tauri::AppHandle,
    queue: State<'_, Arc<JobQueue>>,
    project_id: Option<String>,
    node_home: Option<String>,
) -> Result<ToolchainInfo, String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let env = match project_id.as_deref().or(global.active_project_id.as_deref()) {
        Some(id) => {
            let p = find_project(&store, id).ok_or_else(|| "project not found".to_string())?;
            RuntimeEnv::merge(&global, p).with_node_home_override(node_home.as_deref())
        }
        None => RuntimeEnv {
            java_home: global.java_home,
            maven_home: global.maven_home,
            node_home: global.node_home,
            maven_repo_url: global.maven_repo_url,
            maven_local_repo: global.maven_local_repo,
            workspace_path: String::new(),
        }
        .with_node_home_override(node_home.as_deref()),
    };
    queue
        .run(
            "toolchain",
            "探测工具链",
            Some("toolchain:detect".into()),
            JobPriority::Normal,
            false,
            move |_| Ok(detect_toolchain(&env)),
        )
        .await
}

#[tauri::command]
async fn discover_toolchain_homes_cmd(
    app: tauri::AppHandle,
    queue: State<'_, Arc<JobQueue>>,
    force: Option<bool>,
) -> Result<DiscoverToolchainHomes, String> {
    let force = force.unwrap_or(false);
    let dir = data_dir(&app)?;
    if !force {
        if let Some(cached) = toolchain_cache::load_if_fresh(&dir)? {
            return Ok(cached);
        }
    }
    let dir_bg = dir.clone();
    queue
        .run(
            "toolchain",
            "发现工具链路径",
            Some("toolchain:discover".into()),
            JobPriority::Normal,
            false,
            move |_| {
                let homes = discover_toolchain_homes();
                let _ = toolchain_cache::save(&dir_bg, &homes);
                Ok(homes)
            },
        )
        .await
}

#[tauri::command]
fn get_scan_cache(app: tauri::AppHandle, project_id: String) -> Result<Option<ScanCache>, String> {
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    let dir = data_dir(&app)?;
    match scan_cache::load_scan_cache(&dir, &project_id)? {
        Some(cache) if cache.root_path == project.root_path => Ok(Some(cache)),
        Some(_) => {
            let _ = scan_cache::clear_scan_cache(&dir, &project_id);
            Ok(None)
        }
        None => Ok(None),
    }
}

#[tauri::command]
fn save_scan_cache_cmd(app: tauri::AppHandle, cache: ScanCache) -> Result<(), String> {
    scan_cache::save_scan_cache(&data_dir(&app)?, &cache)
}

/// Scan via JobQueue; emits `scan-progress` for the status bar.
#[tauri::command]
async fn scan_project(
    app: tauri::AppHandle,
    queue: State<'_, Arc<JobQueue>>,
    project_id: String,
) -> Result<Vec<Launchable>, String> {
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id)
        .ok_or_else(|| "project not found".to_string())?
        .clone();
    let sink = sink_from_app(&app);
    let dir = data_dir(&app)?;
    let project_id_bg = project_id.clone();
    let root_path = project.root_path.clone();
    let title = format!("扫描 · {}", project.name);

    queue
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
                });
                match units {
                    Ok(u) => {
                        if let Err(e) =
                            scan_cache::save_units_after_scan(&dir, &project_id_bg, &root_path, &u)
                        {
                            eprintln!("save scan cache: {e}");
                        }
                        Ok(u)
                    }
                    Err(e) => {
                        sink.emit_scan_progress(ScanProgress {
                            project_id: project_id_bg,
                            phase: "error".into(),
                            current: 0,
                            total: None,
                            found: 0,
                            message: format!("扫描失败: {e}"),
                        });
                        Err(e)
                    }
                }
            },
        )
        .await
}

#[tauri::command]
async fn start_unit(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    queue: State<'_, Arc<JobQueue>>,
    project_id: String,
    unit: Launchable,
    spring_profile: Option<String>,
    node_home: Option<String>,
) -> Result<(), String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    let env = RuntimeEnv::merge(&global, project).with_node_home_override(node_home.as_deref());
    let build_mode = project.build_before_run;
    let hot_reload = project.hot_reload && project.hot_reload_restart;
    let mgr = Arc::clone(&*mgr);
    let sink = sink_from_app(&app);
    let title = format!("启动 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    queue
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
        .await
}

#[tauri::command]
async fn hot_reload_compile(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    queue: State<'_, Arc<JobQueue>>,
    project_id: String,
    unit: Launchable,
    changed_path: String,
) -> Result<String, String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    if !project.hot_reload {
        return Err("hot reload is disabled for this project".into());
    }
    let env = RuntimeEnv::merge(&global, project);
    let root_path = project.root_path.clone();
    let restart = project.hot_reload_restart;
    let mgr = Arc::clone(&*mgr);
    let sink = sink_from_app(&app);
    let title = format!("热更新编译 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    queue
        .run(
            "compile",
            &title,
            Some(key),
            JobPriority::High,
            true,
            move |ctx| {
                ctx.set_message("热更新编译…");
                mgr.hot_reload_compile(&sink, &env, &unit, &root_path, &changed_path, restart)
            },
        )
        .await
}

#[tauri::command]
async fn build_unit(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    queue: State<'_, Arc<JobQueue>>,
    project_id: String,
    unit: Launchable,
    force: bool,
) -> Result<(), String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    let env = RuntimeEnv::merge(&global, project);
    let mgr = Arc::clone(&*mgr);
    let sink = sink_from_app(&app);
    let title = format!("构建 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    queue
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
        .await
}

#[tauri::command]
async fn install_local_deps(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    queue: State<'_, Arc<JobQueue>>,
    project_id: String,
    unit: Launchable,
) -> Result<(), String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    let env = RuntimeEnv::merge(&global, project);
    let mgr = Arc::clone(&*mgr);
    let sink = sink_from_app(&app);
    let title = format!("安装依赖 · {}", unit.name);
    let key = format!("compile:{}", unit.id);
    queue
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
        .await
}

#[tauri::command]
fn stop_unit(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    unit_id: String,
) -> Result<(), String> {
    mgr.stop(&sink_from_app(&app), &unit_id)
}

#[tauri::command]
fn term_write(
    mgr: State<'_, Arc<ProcessManager>>,
    unit_id: String,
    data: String,
) -> Result<(), String> {
    mgr.term_write(&unit_id, &data)
}

#[tauri::command]
fn term_resize(
    mgr: State<'_, Arc<ProcessManager>>,
    unit_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    mgr.term_resize(&unit_id, cols, rows)
}

#[tauri::command]
fn list_running(mgr: State<'_, Arc<ProcessManager>>) -> Vec<ModuleRuntime> {
    mgr.list_running()
}

#[tauri::command]
fn start_hot_reload_watch(
    app: tauri::AppHandle,
    watch: State<'_, Arc<HotReloadWatchHub>>,
    project_id: String,
    root_path: String,
) -> Result<(), String> {
    watch.start(sink_from_app(&app), project_id, root_path)
}

#[tauri::command]
fn stop_hot_reload_watch(
    watch: State<'_, Arc<HotReloadWatchHub>>,
    project_id: Option<String>,
) -> Result<(), String> {
    watch.stop(project_id.as_deref());
    Ok(())
}

#[tauri::command]
fn list_jobs(queue: State<'_, Arc<JobQueue>>) -> JobList {
    queue.list()
}

#[tauri::command]
fn cancel_job(queue: State<'_, Arc<JobQueue>>, job_id: String) -> Result<(), String> {
    queue.cancel(&job_id)
}

#[tauri::command]
fn clear_job_history(queue: State<'_, Arc<JobQueue>>) -> Result<(), String> {
    queue.clear_history();
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mgr = Arc::new(ProcessManager::new());
    let watch = Arc::new(HotReloadWatchHub::new());
    let queue = Arc::new(JobQueue::new());
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(mgr)
        .manage(watch)
        .manage(queue)
        .setup(|app| {
            if let Some(q) = app.try_state::<Arc<JobQueue>>() {
                q.set_sink(sink_from_app(app.handle()));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_global_settings,
            save_global_settings,
            list_projects,
            create_project,
            update_project,
            delete_project,
            set_active_project,
            detect_toolchain_cmd,
            discover_toolchain_homes_cmd,
            get_scan_cache,
            save_scan_cache_cmd,
            scan_project,
            start_unit,
            hot_reload_compile,
            build_unit,
            install_local_deps,
            stop_unit,
            term_write,
            term_resize,
            list_running,
            start_hot_reload_watch,
            stop_hot_reload_watch,
            list_jobs,
            cancel_job,
            clear_job_history,
            file_io::read_text_file,
            file_io::read_image_data_url,
            file_io::write_text_file,
            file_io::list_dir,
            search::search_everywhere,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
