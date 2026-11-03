mod build_cache;
mod classpath_jar;
mod deps;
mod file_io;
mod git;
mod hot_reload_watch;
mod search;
mod model;
mod process_mgr;
mod pty_session;
mod scan_cache;
mod scanner;
mod store;
mod toolchain;
mod toolchain_cache;

use hot_reload_watch::HotReloadWatchHub;
use model::{GlobalSettings, Launchable, Project, RuntimeEnv, ScanCache, ScanProgress};
use process_mgr::{ModuleRuntime, ProcessManager};
use std::sync::Arc;
use store::{
    find_project, find_project_mut, load_global, load_projects, migrate_if_needed, save_global,
    save_projects,
};
use tauri::{AppHandle, Emitter, State};
use toolchain::{
    detect_toolchain, discover_toolchain_homes, DiscoverToolchainHomes, ToolchainInfo,
};

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
    let _ = scan_cache::clear_scan_cache(&app, &project_id);
    Ok(())
}

#[tauri::command]
fn set_active_project(app: tauri::AppHandle, project_id: Option<String>) -> Result<(), String> {
    let mut global = load_global(&app)?;
    global.active_project_id = project_id;
    save_global(&app, &global)
}

#[tauri::command]
fn detect_toolchain_cmd(
    app: tauri::AppHandle,
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
    Ok(detect_toolchain(&env))
}

/// Fast env/PATH/known-folder probe for JAVA_HOME / MAVEN_HOME / NODE_HOME (no full-disk walk).
/// Results are cached on disk (~24h) unless `force` is true.
#[tauri::command]
async fn discover_toolchain_homes_cmd(
    app: tauri::AppHandle,
    force: Option<bool>,
) -> Result<DiscoverToolchainHomes, String> {
    let force = force.unwrap_or(false);
    if !force {
        if let Some(cached) = toolchain_cache::load_if_fresh(&app)? {
            return Ok(cached);
        }
    }
    let homes = tauri::async_runtime::spawn_blocking(discover_toolchain_homes)
        .await
        .map_err(|e| format!("discover failed: {e}"))?;
    let _ = toolchain_cache::save(&app, &homes);
    Ok(homes)
}

#[tauri::command]
fn get_scan_cache(app: tauri::AppHandle, project_id: String) -> Result<Option<ScanCache>, String> {
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    match scan_cache::load_scan_cache(&app, &project_id)? {
        Some(cache) if cache.root_path == project.root_path => Ok(Some(cache)),
        Some(_) => {
            // Root changed — drop stale cache
            let _ = scan_cache::clear_scan_cache(&app, &project_id);
            Ok(None)
        }
        None => Ok(None),
    }
}

#[tauri::command]
fn save_scan_cache_cmd(app: tauri::AppHandle, cache: ScanCache) -> Result<(), String> {
    scan_cache::save_scan_cache(&app, &cache)
}

fn emit_scan_progress(app: &AppHandle, progress: ScanProgress) {
    let _ = app.emit("scan-progress", progress);
}

/// Scan on a blocking pool thread; emits `scan-progress` for the status bar.
#[tauri::command]
async fn scan_project(app: tauri::AppHandle, project_id: String) -> Result<Vec<Launchable>, String> {
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id)
        .ok_or_else(|| "project not found".to_string())?
        .clone();
    let app_bg = app.clone();
    let project_id_bg = project_id.clone();
    let root_path = project.root_path.clone();

    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut last_emit = std::time::Instant::now()
            .checked_sub(std::time::Duration::from_millis(200))
            .unwrap_or_else(std::time::Instant::now);
        let units = scanner::scan_project_with_progress(&project, &mut |p| {
            // Throttle UI events (~5/s) except phase changes / done
            let force = p.phase == "done"
                || p.phase == "error"
                || p.current == 0
                || last_emit.elapsed().as_millis() >= 180;
            if force {
                last_emit = std::time::Instant::now();
                emit_scan_progress(&app_bg, p);
            }
        });
        match units {
            Ok(u) => {
                if let Err(e) =
                    scan_cache::save_units_after_scan(&app_bg, &project_id_bg, &root_path, &u)
                {
                    eprintln!("save scan cache: {e}");
                }
                Ok(u)
            }
            Err(e) => {
                emit_scan_progress(
                    &app_bg,
                    ScanProgress {
                        project_id: project_id_bg,
                        phase: "error".into(),
                        current: 0,
                        total: None,
                        found: 0,
                        message: format!("扫描失败: {e}"),
                    },
                );
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| format!("scan task join: {e}"))?;

    result
}

#[tauri::command]
fn start_unit(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    project_id: String,
    unit: Launchable,
    spring_profile: Option<String>,
    node_home: Option<String>,
) -> Result<(), String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    let env = RuntimeEnv::merge(&global, project).with_node_home_override(node_home.as_deref());
    mgr.start(
        app,
        env,
        unit,
        project.build_before_run,
        spring_profile,
        // DevTools only when user explicitly opts into context restart
        project.hot_reload && project.hot_reload_restart,
    )
}

#[tauri::command]
fn hot_reload_compile(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
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
    mgr.hot_reload_compile(
        &app,
        &env,
        &unit,
        &project.root_path,
        &changed_path,
        project.hot_reload_restart,
    )
}

#[tauri::command]
fn build_unit(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    project_id: String,
    unit: Launchable,
    force: bool,
) -> Result<(), String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    let env = RuntimeEnv::merge(&global, project);
    mgr.build_unit(app, env, unit, force)
}

#[tauri::command]
fn install_local_deps(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    project_id: String,
    unit: Launchable,
) -> Result<(), String> {
    let global = load_global(&app)?;
    let store = load_projects(&app)?;
    let project = find_project(&store, &project_id).ok_or_else(|| "project not found".to_string())?;
    let env = RuntimeEnv::merge(&global, project);
    mgr.install_local_deps(app, env, unit)
}

#[tauri::command]
fn stop_unit(
    app: tauri::AppHandle,
    mgr: State<'_, Arc<ProcessManager>>,
    unit_id: String,
) -> Result<(), String> {
    mgr.stop(&app, &unit_id)
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
    watch.start(app, project_id, root_path)
}

#[tauri::command]
fn stop_hot_reload_watch(
    watch: State<'_, Arc<HotReloadWatchHub>>,
    project_id: Option<String>,
) -> Result<(), String> {
    watch.stop(project_id.as_deref());
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mgr = Arc::new(ProcessManager::new());
    let watch = Arc::new(HotReloadWatchHub::new());
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(mgr)
        .manage(watch)
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
            file_io::read_text_file,
            file_io::read_image_data_url,
            file_io::write_text_file,
            file_io::list_dir,
            search::search_everywhere,
            git::git_repo_summary,
            git::git_status,
            git::git_diff,
            git::git_stage,
            git::git_unstage,
            git::git_discard,
            git::git_commit,
            git::git_branches,
            git::git_checkout,
            git::git_create_branch,
            git::git_log,
            git::git_show,
            git::git_fetch,
            git::git_pull,
            git::git_push,
            git::git_merge,
            git::git_merge_abort,
            git::git_merge_continue,
            git::git_conflict_paths,
            git::git_conflict_sides,
            git::git_write_file,
            git::git_stash_list,
            git::git_stash_push,
            git::git_stash_apply,
            git::git_stash_drop,
            git::git_default_merge_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
