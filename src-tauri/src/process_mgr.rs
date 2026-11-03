use crate::build_cache::{
    ensure_classes_exist, is_module_stale, write_compile_stamp,
};
use crate::classpath_jar::shorten_classpath_arg;
use crate::deps::{
    normalize_path, resolve_local_deps, resolve_local_install_order, strip_block,
};
use crate::model::{BuildBeforeRun, Launchable, ProjectKind, RuntimeEnv};
use crate::toolchain::{
    apply_env, command_for, effective_maven_local_repo, resolve_java_bin, resolve_javac_bin,
    resolve_mvn_bin, resolve_pm_invocation, with_maven_settings_args,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

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
pub struct ModuleRuntime {
    pub unit_id: String,
    pub status: ModuleStatus,
    pub pid: Option<u32>,
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

enum ManagedProcess {
    Piped { child: Child },
    Pty(crate::pty_session::PtySession),
}

pub struct ProcessManager {
    inner: Mutex<HashMap<String, ManagedProcess>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn list_running(&self) -> Vec<ModuleRuntime> {
        let guard = self.inner.lock().unwrap();
        guard
            .iter()
            .map(|(id, mp)| {
                let pid = match mp {
                    ManagedProcess::Piped { child } => Some(child.id()),
                    ManagedProcess::Pty(s) => {
                        if s.pid == 0 {
                            None
                        } else {
                            Some(s.pid)
                        }
                    }
                };
                ModuleRuntime {
                    unit_id: id.clone(),
                    status: ModuleStatus::Running,
                    pid,
                }
            })
            .collect()
    }

    pub fn term_write(&self, unit_id: &str, data: &str) -> Result<(), String> {
        let guard = self.inner.lock().unwrap();
        match guard.get(unit_id) {
            Some(ManagedProcess::Pty(s)) => s.write(data),
            Some(_) => Err("unit is not an embedded terminal session".into()),
            None => Err("unit not running".into()),
        }
    }

    pub fn term_resize(&self, unit_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let guard = self.inner.lock().unwrap();
        match guard.get(unit_id) {
            Some(ManagedProcess::Pty(s)) => s.resize(cols, rows),
            Some(_) => Ok(()),
            None => Ok(()),
        }
    }

    pub fn start(
        self: &Arc<Self>,
        app: AppHandle,
        env: RuntimeEnv,
        unit: Launchable,
        build_mode: BuildBeforeRun,
        spring_profile: Option<String>,
        hot_reload: bool,
    ) -> Result<(), String> {
        let unit_id = unit.id.clone();
        // Reclaim leftover marker/process (e.g. frontend HMR cleared UI state but Rust still holds pid).
        self.reclaim_unit(&unit_id);

        let _ = app.emit(
            "module-status",
            StatusEvent {
                unit_id: unit_id.clone(),
                status: ModuleStatus::Starting,
                pid: None,
                message: Some("starting".into()),
            },
        );

        let mgr = Arc::clone(self);
        let app_bg = app.clone();
        let env_bg = env.clone();
        let unit_bg = unit.clone();
        let profile = spring_profile
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| unit.selected_spring_profile.clone());

        {
            let mut guard = self.inner.lock().unwrap();
            if guard.contains_key(&unit_id) {
                // Extremely rare race: another start just inserted
                return Err("unit already running".to_string());
            }
            let marker = spawn_marker(&env)?;
            guard.insert(
                unit_id.clone(),
                ManagedProcess::Piped { child: marker },
            );
        }

        std::thread::spawn(move || {
            let log = |stream: &str, line: String| {
                let _ = app_bg.emit(
                    "module-log",
                    LogEvent {
                        unit_id: unit_bg.id.clone(),
                        stream: stream.into(),
                        line,
                    },
                );
            };

            let result = match unit_bg.kind {
                ProjectKind::SpringBoot => prepare_and_run_spring_boot(
                    &mgr,
                    &app_bg,
                    &env_bg,
                    &unit_bg,
                    build_mode,
                    &profile,
                    hot_reload,
                    &log,
                ),
                ProjectKind::Vue => prepare_and_run_vue(&mgr, &app_bg, &env_bg, &unit_bg, &log),
            };

            if let Err(e) = result {
                log("stderr", format!("[launcher] {e}"));
                finish_failed(&mgr, &app_bg, &unit_bg.id);
            }
        });

        Ok(())
    }

    /// Compile the Maven module that owns `changed_path`.
    /// If `restart` is true, also touch DevTools `.reloadtrigger` (full ApplicationContext restart).
    pub fn hot_reload_compile(
        &self,
        app: &AppHandle,
        env: &RuntimeEnv,
        unit: &Launchable,
        project_root: &str,
        changed_path: &str,
        restart: bool,
    ) -> Result<String, String> {
        if unit.kind != ProjectKind::SpringBoot {
            return Err("hot reload compile is only for Spring Boot units".into());
        }
        if !self.is_running(&unit.id) {
            return Err("unit is not running".into());
        }
        let root = PathBuf::from(normalize_path(project_root));
        let changed = PathBuf::from(changed_path);
        let changed_canon = changed
            .canonicalize()
            .unwrap_or_else(|_| changed.clone());
        let root_canon = root
            .canonicalize()
            .map_err(|e| format!("project root: {e}"))?;
        if !changed_canon.starts_with(&root_canon) {
            return Err("changed file outside project root".into());
        }

        // canonicalize() on Windows yields `\\?\D:\...`; javac/mvn choke on that (→ //?/D:/...).
        let module_dir = find_maven_module_dir(&changed_canon)
            .ok_or_else(|| "no pom.xml found for changed file".to_string())?;
        let module_dir = PathBuf::from(normalize_path(&module_dir.to_string_lossy()));
        let changed_file = PathBuf::from(normalize_path(&changed_canon.to_string_lossy()));

        let log = |stream: &str, line: String| {
            let _ = app.emit(
                "module-log",
                LogEvent {
                    unit_id: unit.id.clone(),
                    stream: stream.into(),
                    line,
                },
            );
        };

        let ext = changed_file
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let is_sources = matches!(ext.as_str(), "java" | "kt");
        let is_config = matches!(ext.as_str(), "yml" | "yaml" | "properties" | "xml");
        if !is_sources && !is_config {
            return Err(format!("unsupported hot-reload extension: .{ext}"));
        }

        if is_sources && ext == "java" {
            // Prefer single-file javac (seconds) over Maven full-module recompile (7652 files).
            match javac_single_java_file(
                env,
                &root,
                &module_dir,
                &changed_file,
                &unit.id,
                app,
                &log,
            ) {
                Ok(()) => {
                    let _ = write_compile_stamp(&module_dir);
                }
                Err(e) => {
                    log(
                        "stderr",
                        format!(
                            "[launcher] hot-reload javac failed ({e}); fallback incremental mvn compile"
                        ),
                    );
                    if !mvn_incremental_compile(env, &module_dir, &unit.id, app, &log)? {
                        return Err("hot-reload mvn compile failed".into());
                    }
                    let _ = write_compile_stamp(&module_dir);
                }
            }
        } else if is_sources {
            // Kotlin / other: Maven with file-level incremental (not whole-module)
            if !mvn_incremental_compile(env, &module_dir, &unit.id, app, &log)? {
                return Err("hot-reload mvn compile failed".into());
            }
            let _ = write_compile_stamp(&module_dir);
        } else {
            log(
                "stdout",
                format!(
                    "[launcher] hot-reload process-resources {}",
                    module_dir.display()
                ),
            );
            let mvn = resolve_mvn_bin(env);
            let args = vec!["process-resources".into()];
            let ok = run_command_streaming(env, &module_dir, &mvn, &args, &unit.id, app)?;
            if !ok {
                return Err("hot-reload process-resources failed".into());
            }
        }

        if restart {
            let unit_classes = Path::new(&normalize_path(&unit.path))
                .join("target")
                .join("classes");
            touch_devtools_trigger(&unit_classes)?;
            if module_dir != Path::new(&normalize_path(&unit.path)) {
                let dep_classes = module_dir.join("target").join("classes");
                let _ = touch_devtools_trigger(&dep_classes);
            }
            log(
                "stdout",
                "[launcher] hot-reload: classes updated, DevTools should restart context".into(),
            );
        } else {
            log(
                "stdout",
                "[launcher] hot-reload: classes updated (compile only, no context restart)".into(),
            );
        }
        Ok(format!("compiled {}", module_dir.display()))
    }

    pub fn is_running(&self, unit_id: &str) -> bool {
        let guard = self.inner.lock().unwrap();
        guard.contains_key(unit_id)
    }

    /// Compile unit + local deps (no run). `force` ignores up-to-date checks.
    pub fn build_unit(
        self: &Arc<Self>,
        app: AppHandle,
        env: RuntimeEnv,
        unit: Launchable,
        force: bool,
    ) -> Result<(), String> {
        if unit.kind != ProjectKind::SpringBoot {
            return Err("build is only supported for Spring Boot units".into());
        }
        let unit_id = unit.id.clone();
        ensure_log_slot(&app, &unit_id);
        std::thread::spawn(move || {
            let log = |stream: &str, line: String| {
                let _ = app.emit(
                    "module-log",
                    LogEvent {
                        unit_id: unit.id.clone(),
                        stream: stream.into(),
                        line,
                    },
                );
            };
            let mode = if force {
                BuildBeforeRun::Always
            } else {
                BuildBeforeRun::Smart
            };
            if let Err(e) = compile_spring_modules(&env, &unit, mode, &log, &app) {
                log("stderr", format!("[launcher] build failed: {e}"));
                let _ = app.emit(
                    "module-status",
                    StatusEvent {
                        unit_id: unit.id.clone(),
                        status: ModuleStatus::Failed,
                        pid: None,
                        message: Some(e),
                    },
                );
            } else {
                log("stdout", "[launcher] build finished".into());
                let _ = app.emit(
                    "module-status",
                    StatusEvent {
                        unit_id: unit.id.clone(),
                        status: ModuleStatus::Stopped,
                        pid: None,
                        message: Some("build ok".into()),
                    },
                );
            }
        });
        Ok(())
    }

    /// Force `mvn install -DskipTests` for local deps of a Spring Boot unit (advanced).
    pub fn install_local_deps(
        self: &Arc<Self>,
        app: AppHandle,
        env: RuntimeEnv,
        unit: Launchable,
    ) -> Result<(), String> {
        if unit.kind != ProjectKind::SpringBoot {
            return Err("install deps is only for Spring Boot units".into());
        }
        std::thread::spawn(move || {
            let log = |stream: &str, line: String| {
                let _ = app.emit(
                    "module-log",
                    LogEvent {
                        unit_id: unit.id.clone(),
                        stream: stream.into(),
                        line,
                    },
                );
            };
            let module_path = normalize_path(&unit.path);
            let workspace = normalize_path(&env.workspace_path);
            match resolve_local_install_order(Path::new(&workspace), Path::new(&module_path)) {
                Ok(order) => {
                    for dep_dir in order {
                        let name = dep_dir
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        log(
                            "stdout",
                            format!("[launcher] mvn install -DskipTests @ {name}"),
                        );
                        match run_mvn_streaming(
                            &env,
                            &dep_dir,
                            vec!["install".into(), "-DskipTests".into()],
                            &unit.id,
                            &app,
                        ) {
                            Ok(true) => {
                                let _ = write_compile_stamp(&dep_dir);
                                log("stdout", format!("[launcher] install OK: {name}"));
                            }
                            Ok(false) => {
                                log("stderr", format!("[launcher] install FAILED: {name}"));
                                return;
                            }
                            Err(e) => {
                                log("stderr", format!("[launcher] {e}"));
                                return;
                            }
                        }
                    }
                    log("stdout", "[launcher] install local deps finished".into());
                }
                Err(e) => log("stderr", format!("[launcher] {e}")),
            }
        });
        Ok(())
    }

    pub fn stop(&self, app: &AppHandle, unit_id: &str) -> Result<(), String> {
        if !self.reclaim_unit(unit_id) {
            // Already gone — still notify UI so status stays consistent
            let _ = app.emit(
                "module-status",
                StatusEvent {
                    unit_id: unit_id.to_string(),
                    status: ModuleStatus::Stopped,
                    pid: None,
                    message: Some("not running".into()),
                },
            );
            return Ok(());
        }

        let _ = app.emit(
            "module-status",
            StatusEvent {
                unit_id: unit_id.to_string(),
                status: ModuleStatus::Stopped,
                pid: None,
                message: Some("stopped by user".into()),
            },
        );
        Ok(())
    }

    /// Remove map entry and kill process tree if present. Returns true if an entry existed.
    fn reclaim_unit(&self, unit_id: &str) -> bool {
        let mp = {
            let mut guard = self.inner.lock().unwrap();
            match guard.remove(unit_id) {
                Some(mp) => mp,
                None => return false,
            }
        };
        match mp {
            ManagedProcess::Piped { mut child } => {
                let pid = child.id();
                let _ = kill_process_tree(pid);
                let _ = child.try_wait();
                let _ = child.kill();
                let _ = child.wait();
            }
            ManagedProcess::Pty(session) => {
                session.kill_tree();
            }
        }
        true
    }
}

fn ensure_log_slot(app: &AppHandle, unit_id: &str) {
    let _ = app.emit(
        "module-status",
        StatusEvent {
            unit_id: unit_id.to_string(),
            status: ModuleStatus::Starting,
            pid: None,
            message: Some("building".into()),
        },
    );
}

fn compile_spring_modules(
    env: &RuntimeEnv,
    unit: &Launchable,
    mode: BuildBeforeRun,
    log: &dyn Fn(&str, String),
    app: &AppHandle,
) -> Result<Vec<(String, PathBuf)>, String> {
    let module_path = normalize_path(&unit.path);
    let workspace = normalize_path(&env.workspace_path);
    let mvn = resolve_mvn_bin(env);
    let local_deps = resolve_local_deps(Path::new(&workspace), Path::new(&module_path))?;

    // Local workspace deps: consumer `mvn compile` needs GAV in local repo.
    // When sources are up-to-date, never run full `mvn install` (that recompiles everything);
    // use jar:jar + install:install-file from existing target/classes instead.
    let build_one = |dir: &Path, name: &str, install: bool| -> Result<(), String> {
        let goal = if install { "install" } else { "compile" };
        match mode {
            BuildBeforeRun::Never => {
                ensure_classes_exist(dir)?;
                Ok(())
            }
            BuildBeforeRun::Always => {
                // Force module participation, but still compile only stale .java vs .class
                // (Maven default useIncrementalCompilation=true recompiles ALL 7k+ sources).
                log(
                    "stdout",
                    format!(
                        "[launcher] mvn {goal} -DskipTests (file-incremental) @ {name}"
                    ),
                );
                let ok = run_mvn_file_incremental(env, dir, goal, &unit.id, app)?;
                if !ok {
                    return Err(format!("{goal} FAILED: {name}"));
                }
                write_compile_stamp(dir)?;
                Ok(())
            }
            BuildBeforeRun::Smart => {
                if !is_module_stale(dir) {
                    if !install {
                        return Ok(());
                    }
                    let gav = parse_project_gav(dir)?;
                    let m2_jar = m2_artifact_jar_path(env, &gav);
                    if m2_jar.is_file() {
                        return Ok(());
                    }
                    log(
                        "stdout",
                        format!(
                            "[launcher] {name} up-to-date — install-file to .m2 (no recompile); missing {}",
                            m2_jar.display()
                        ),
                    );
                    install_local_dep_fast(env, dir, &gav, &mvn, &unit.id, app, log)?;
                    return Ok(());
                }
                log(
                    "stdout",
                    format!(
                        "[launcher] mvn {goal} -DskipTests (file-incremental) @ {name}"
                    ),
                );
                let ok = run_mvn_file_incremental(env, dir, goal, &unit.id, app)?;
                if !ok {
                    return Err(format!("{goal} FAILED: {name}"));
                }
                write_compile_stamp(dir)?;
                Ok(())
            }
        }
    };

    for (artifact_id, dep_dir) in &local_deps {
        build_one(dep_dir, artifact_id, true)?;
    }
    let self_name = Path::new(&module_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "module".into());
    // Running module: compile is enough for java -cp; install optional.
    build_one(Path::new(&module_path), &self_name, false)?;

    Ok(local_deps)
}

#[derive(Debug, Clone)]
struct ProjectGav {
    group_id: String,
    artifact_id: String,
    version: String,
}

fn parse_project_gav(module_dir: &Path) -> Result<ProjectGav, String> {
    let pom_path = module_dir.join("pom.xml");
    let pom = fs::read_to_string(&pom_path)
        .map_err(|e| format!("read {}: {e}", pom_path.display()))?;
    let mut cleaned = strip_block(&pom, "parent");
    cleaned = strip_block(&cleaned, "dependencies");
    cleaned = strip_block(&cleaned, "dependencyManagement");
    cleaned = strip_block(&cleaned, "build");
    cleaned = strip_block(&cleaned, "profiles");
    cleaned = strip_block(&cleaned, "properties");

    let tag = |name: &str| -> Option<String> {
        let re = Regex::new(&format!(r"(?s)<{name}>\s*([^<]+?)\s*</{name}>")).ok()?;
        re.captures(&cleaned)
            .map(|c| c[1].trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let artifact_id = tag("artifactId").unwrap_or_else(|| {
        module_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    });
    Ok(ProjectGav {
        group_id: tag("groupId").unwrap_or_else(|| "com.gdzc".into()),
        artifact_id,
        version: tag("version").unwrap_or_else(|| "1.0.0".into()),
    })
}

fn m2_artifact_jar_path(env: &RuntimeEnv, gav: &ProjectGav) -> PathBuf {
    let repo = effective_maven_local_repo(env);
    let rel: PathBuf = gav.group_id.split('.').collect();
    repo.join(rel)
        .join(&gav.artifact_id)
        .join(&gav.version)
        .join(format!("{}-{}.jar", gav.artifact_id, gav.version))
}

fn module_target_jar(module_dir: &Path, gav: &ProjectGav) -> PathBuf {
    module_dir
        .join("target")
        .join(format!("{}-{}.jar", gav.artifact_id, gav.version))
}

/// Package existing classes (no compiler) and install into local .m2.
fn install_local_dep_fast(
    env: &RuntimeEnv,
    module_dir: &Path,
    gav: &ProjectGav,
    mvn: &Path,
    unit_id: &str,
    app: &AppHandle,
    log: &dyn Fn(&str, String),
) -> Result<(), String> {
    ensure_classes_exist(module_dir)?;
    let target_jar = module_target_jar(module_dir, gav);
    if !target_jar.is_file() {
        log(
            "stdout",
            format!(
                "[launcher] packaging {} (jar:jar, no compile)…",
                gav.artifact_id
            ),
        );
        let ok = run_command_streaming(
            env,
            module_dir,
            mvn,
            &["-q".into(), "jar:jar".into()],
            unit_id,
            app,
        )?;
        if !ok {
            return Err(format!("jar:jar FAILED: {}", gav.artifact_id));
        }
        if !target_jar.is_file() {
            return Err(format!(
                "jar:jar did not produce {}",
                target_jar.display()
            ));
        }
    }

    let args = vec![
        "-q".into(),
        "install:install-file".into(),
        format!("-Dfile={}", target_jar.display()),
        format!("-DgroupId={}", gav.group_id),
        format!("-DartifactId={}", gav.artifact_id),
        format!("-Dversion={}", gav.version),
        "-Dpackaging=jar".into(),
    ];
    log(
        "stdout",
        format!(
            "[launcher] install:install-file {}:{}:{}",
            gav.group_id, gav.artifact_id, gav.version
        ),
    );
    let ok = run_command_streaming(env, module_dir, mvn, &args, unit_id, app)?;
    if !ok {
        return Err(format!("install:install-file FAILED: {}", gav.artifact_id));
    }
    let m2_jar = m2_artifact_jar_path(env, gav);
    if !m2_jar.is_file() {
        return Err(format!(
            "install:install-file reported OK but jar missing at {}",
            m2_jar.display()
        ));
    }
    Ok(())
}

fn normalize_spring_profile(profile: &str) -> Option<String> {
    let p = profile.trim();
    if p.is_empty() || p.eq_ignore_ascii_case("application") || p.eq_ignore_ascii_case("default")
    {
        None
    } else {
        Some(p.to_string())
    }
}

fn prepare_and_run_spring_boot(
    mgr: &Arc<ProcessManager>,
    app: &AppHandle,
    env: &RuntimeEnv,
    unit: &Launchable,
    build_mode: BuildBeforeRun,
    spring_profile: &str,
    hot_reload: bool,
    log: &dyn Fn(&str, String),
) -> Result<(), String> {
    // IDEA-like: compile only stale modules → java -cp (module + local dep classes + jars)
    let module_path = normalize_path(&unit.path);
    let mvn = resolve_mvn_bin(env);

    let local_deps = compile_spring_modules(env, unit, build_mode, log, app)?;

    let main_class = unit
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('.'));

    let active = normalize_spring_profile(spring_profile);
    clear_marker(mgr, &unit.id);

    if let Some(main) = main_class {
        let mut classpath = build_runtime_classpath(
            env,
            Path::new(&module_path),
            &local_deps,
            &unit.id,
            app,
        )?;
        if hot_reload {
            match ensure_devtools_on_classpath(env, Path::new(&module_path), &classpath, log) {
                Ok(cp) => classpath = cp,
                Err(e) => log("stderr", format!("[launcher] hot-reload DevTools: {e}")),
            }
            // ShardingSphere CLASS_BASED 用 Class.forName(调用方 CL) 加载业务分片类；
            // DevTools 默认把第三方 jar 放 base CL、业务类放 Restart CL → SpringUtil NPE。
            // 把 shardingsphere*.jar 纳入 Restart CL，与业务代码同加载器。
            if let Err(e) = write_devtools_sharding_bridge_props(Path::new(&module_path), log) {
                log(
                    "stderr",
                    format!("[launcher] hot-reload spring-devtools.properties: {e}"),
                );
            }
        }
        // Windows CreateProcess limit (~8191) → OS error 206; use classpath JAR like IDEA.
        let cp_arg = shorten_classpath_arg(Path::new(&module_path), &classpath, log)?;
        let java = resolve_java_bin(env);
        // Match IDEA: Spring Boot ANSI colors in console
        let mut args = vec![
            "-cp".into(),
            cp_arg.clone(),
            "-Dspring.output.ansi.enabled=ALWAYS".into(),
            "-Dfile.encoding=UTF-8".into(),
        ];
        if hot_reload {
            args.push("-Dspring.devtools.restart.enabled=true".into());
            args.push("-Dspring.devtools.restart.trigger-file=.reloadtrigger".into());
            // Quiet Liveload noise; keep restart
            args.push("-Dspring.devtools.livereload.enabled=false".into());
            log(
                "stdout",
                "[launcher] hot-reload: spring-boot-devtools enabled (context restart on class change)".into(),
            );
        }
        if let Some(ref p) = active {
            args.push(format!("-Dspring.profiles.active={p}"));
        }
        args.push(main.to_string());
        // One command line preview (IDEA-like), not per-jar chatter
        let preview_cp = if cp_arg.len() > 120 {
            format!("{}…", &cp_arg[..120])
        } else {
            cp_arg.clone()
        };
        log(
            "stdout",
            format!(
                "{} -cp {}{} {}",
                java.display(),
                preview_cp,
                active
                    .as_ref()
                    .map(|p| format!(" -Dspring.profiles.active={p}"))
                    .unwrap_or_default(),
                main
            ),
        );
        spawn_long_running(
            mgr,
            app,
            env,
            &unit.id,
            Path::new(&module_path),
            &java,
            &args,
        )
    } else {
        log(
            "stdout",
            "[launcher] main class unknown — fallback mvn spring-boot:run".into(),
        );
        let mut jvm_args = String::from("-Dspring.output.ansi.enabled=ALWAYS");
        if hot_reload {
            jvm_args.push_str(
                " -Dspring.devtools.restart.enabled=true -Dspring.devtools.restart.trigger-file=.reloadtrigger -Dspring.devtools.livereload.enabled=false",
            );
            log(
                "stdout",
                "[launcher] hot-reload: ensure spring-boot-devtools is on the module classpath for spring-boot:run".into(),
            );
        }
        let mut args = vec![
            "spring-boot:run".into(),
            "-DskipTests".into(),
            format!("-Dspring-boot.run.jvmArguments={jvm_args}"),
        ];
        if let Some(ref p) = active {
            args.push(format!("-Dspring-boot.run.profiles={p}"));
        }
        spawn_long_running(
            mgr,
            app,
            env,
            &unit.id,
            Path::new(&module_path),
            &mvn,
            &args,
        )
    }
}

/// Classpath: current + local dep `target/classes` first, then Maven jars minus local artifacts.
fn build_runtime_classpath(
    env: &RuntimeEnv,
    module_dir: &Path,
    local_deps: &[(String, PathBuf)],
    unit_id: &str,
    app: &AppHandle,
) -> Result<String, String> {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let cp_file = module_dir.join(".launcher-classpath.txt");
    let mvn = resolve_mvn_bin(env);
    let args = vec![
        "org.apache.maven.plugins:maven-dependency-plugin:3.6.1:build-classpath".into(),
        format!("-Dmdep.outputFile={}", cp_file.display()),
        format!("-Dmdep.pathSeparator={sep}"),
        "-DincludeScope=runtime".into(),
    ];
    let ok = run_command_streaming(env, module_dir, &mvn, &args, unit_id, app)?;
    if !ok {
        return Err("dependency:build-classpath failed".into());
    }
    let dep_cp = fs::read_to_string(&cp_file)
        .map_err(|e| format!("read classpath file: {e}"))?
        .trim()
        .to_string();
    let _ = fs::remove_file(&cp_file);

    let local_ids: Vec<&str> = local_deps.iter().map(|(id, _)| id.as_str()).collect();
    // Windows paths contain ':' (C:\...), so only split on the OS classpath separator.
    let jar_iter = if cfg!(windows) {
        dep_cp.split(';')
    } else {
        dep_cp.split(':')
    };
    let filtered_jars: Vec<String> = jar_iter
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| !is_local_artifact_jar(p, &local_ids))
        .map(|s| s.to_string())
        .collect();

    let mut parts: Vec<String> = Vec::new();
    let self_classes = module_dir.join("target").join("classes");
    if !self_classes.is_dir() {
        return Err(format!(
            "missing compiled output: {}",
            self_classes.display()
        ));
    }
    parts.push(self_classes.to_string_lossy().to_string());
    for (id, dep_dir) in local_deps {
        let classes = dep_dir.join("target").join("classes");
        if classes.is_dir() {
            parts.push(classes.to_string_lossy().to_string());
        } else {
            return Err(format!(
                "missing local dep classes for {id}: {}",
                classes.display()
            ));
        }
    }
    parts.extend(filtered_jars);
    Ok(parts.join(sep))
}

fn is_local_artifact_jar(jar_path: &str, local_ids: &[&str]) -> bool {
    let name = Path::new(jar_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    local_ids.iter().any(|id| {
        let prefix = format!("{}-", id.to_lowercase());
        name.starts_with(&prefix) && name.ends_with(".jar")
    })
}

const DEVTOOLS_TRIGGER: &str = ".reloadtrigger";
const COMPILE_CP_CACHE: &str = ".launcher-compile-cp.txt";
/// Maven default `useIncrementalCompilation=true` recompiles the **entire** module when any
/// source changes. `false` = only .java newer than their .class (IDEA-like for body edits).
const MVN_FILE_INCREMENTAL: &str = "-Dmaven.compiler.useIncrementalCompilation=false";

fn find_maven_module_dir(path: &Path) -> Option<PathBuf> {
    let mut cur = if path.is_file() {
        path.parent()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    loop {
        if cur.join("pom.xml").is_file() {
            return Some(cur);
        }
        cur = cur.parent()?.to_path_buf();
    }
}

/// Write `META-INF/spring-devtools.properties` so ShardingSphere jars load in the
/// RestartClassLoader (same as project classes). Fixes CLASS_BASED sharding + SpringUtil NPE.
fn write_devtools_sharding_bridge_props(
    module_dir: &Path,
    log: &dyn Fn(&str, String),
) -> Result<(), String> {
    let meta = module_dir
        .join("target")
        .join("classes")
        .join("META-INF");
    fs::create_dir_all(&meta).map_err(|e| format!("mkdir META-INF: {e}"))?;
    let props = meta.join("spring-devtools.properties");
    // Properties file: `\\` → `\`; regex needs `\w` and `\.`
    let content = "\
# Generated by SparkRun when DevTools hot-reload is enabled.\n\
# Keep ShardingSphere in RestartClassLoader with app code (CLASS_BASED + SpringUtil).\n\
restart.include.shardingsphere=/shardingsphere-[\\\\w.-]+\\\\.jar\n\
";
    fs::write(&props, content).map_err(|e| format!("write {}: {e}", props.display()))?;
    log(
        "stdout",
        format!(
            "[launcher] hot-reload: wrote {} (shardingsphere → RestartClassLoader)",
            props.display()
        ),
    );
    Ok(())
}

fn touch_devtools_trigger(classes_dir: &Path) -> Result<(), String> {
    if !classes_dir.is_dir() {
        fs::create_dir_all(classes_dir).map_err(|e| format!("mkdir classes: {e}"))?;
    }
    let trigger = classes_dir.join(DEVTOOLS_TRIGGER);
    fs::write(&trigger, format!("{}\n", chrono_like_millis()))
        .map_err(|e| format!("touch trigger: {e}"))
}

fn chrono_like_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn extract_spring_boot_version(classpath: &str) -> Option<String> {
    // Prefer plain spring-boot-{ver}.jar (not autoconfigure / starter / …)
    let re = Regex::new(
        r"(?i)[/\\]spring-boot-(\d+\.\d+\.\d+(?:\.[A-Za-z0-9]+)?)\.jar(?:$|[;:])",
    )
    .ok()?;
    re.captures(classpath)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

fn path_mtime(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn run_mvn_file_incremental(
    env: &RuntimeEnv,
    module_dir: &Path,
    goal: &str,
    unit_id: &str,
    app: &AppHandle,
) -> Result<bool, String> {
    let mvn = resolve_mvn_bin(env);
    let args = vec![
        goal.to_string(),
        "-DskipTests".into(),
        MVN_FILE_INCREMENTAL.into(),
    ];
    run_command_streaming(env, module_dir, &mvn, &args, unit_id, app)
}

fn mvn_incremental_compile(
    env: &RuntimeEnv,
    module_dir: &Path,
    unit_id: &str,
    app: &AppHandle,
    log: &dyn Fn(&str, String),
) -> Result<bool, String> {
    log(
        "stdout",
        format!(
            "[launcher] hot-reload mvn compile (file-incremental) {}",
            module_dir.display()
        ),
    );
    run_mvn_file_incremental(env, module_dir, "compile", unit_id, app)
}

/// Compile classpath for javac: `target/classes` + local dep classes + Maven jars (cached vs pom).
fn ensure_compile_classpath(
    env: &RuntimeEnv,
    workspace: &Path,
    module_dir: &Path,
    unit_id: &str,
    app: &AppHandle,
    log: &dyn Fn(&str, String),
) -> Result<String, String> {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let module_dir = PathBuf::from(normalize_path(&module_dir.to_string_lossy()));
    let workspace = PathBuf::from(normalize_path(&workspace.to_string_lossy()));
    let cache = module_dir.join(COMPILE_CP_CACHE);
    let pom = module_dir.join("pom.xml");
    let classes = module_dir.join("target").join("classes");
    if !classes.is_dir() {
        return Err(format!(
            "missing {}; run a full Build once first",
            classes.display()
        ));
    }

    let cache_fresh = cache.is_file()
        && pom.is_file()
        && path_mtime(&cache).zip(path_mtime(&pom)).is_some_and(|(c, p)| c >= p);

    let dep_cp = if cache_fresh {
        fs::read_to_string(&cache)
            .map_err(|e| format!("read compile classpath cache: {e}"))?
            .trim()
            .to_string()
    } else {
        log(
            "stdout",
            "[launcher] hot-reload: refreshing compile classpath cache…".into(),
        );
        let tmp = module_dir.join(".launcher-compile-cp.tmp");
        let mvn = resolve_mvn_bin(env);
        let args = vec![
            "org.apache.maven.plugins:maven-dependency-plugin:3.6.1:build-classpath".into(),
            format!("-Dmdep.outputFile={}", tmp.display()),
            format!("-Dmdep.pathSeparator={sep}"),
            "-DincludeScope=compile".into(),
        ];
        let ok = run_command_streaming(env, &module_dir, &mvn, &args, unit_id, app)?;
        if !ok {
            let _ = fs::remove_file(&tmp);
            return Err("dependency:build-classpath (compile) failed".into());
        }
        let dep_cp = fs::read_to_string(&tmp)
            .map_err(|e| format!("read classpath tmp: {e}"))?
            .trim()
            .to_string();
        let _ = fs::remove_file(&tmp);
        fs::write(&cache, &dep_cp).map_err(|e| format!("write compile cp cache: {e}"))?;
        dep_cp
    };

    // Prefer workspace local dep `target/classes` over stale .m2 jars (same as runtime CP).
    let local_deps = resolve_local_deps(&workspace, &module_dir).unwrap_or_default();
    let mut front: Vec<String> = vec![normalize_path(&classes.to_string_lossy())];
    let mut local_ids: Vec<String> = Vec::new();
    for (id, dep_dir) in &local_deps {
        let dep_classes = dep_dir.join("target").join("classes");
        if dep_classes.is_dir() {
            front.push(normalize_path(&dep_classes.to_string_lossy()));
            local_ids.push(id.clone());
        }
    }
    let local_id_refs: Vec<&str> = local_ids.iter().map(|s| s.as_str()).collect();
    let jar_iter = if cfg!(windows) {
        dep_cp.split(';')
    } else {
        dep_cp.split(':')
    };
    let jars: Vec<String> = jar_iter
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| !is_local_artifact_jar(p, &local_id_refs))
        .map(|p| normalize_path(p))
        .collect();

    front.extend(jars);
    Ok(front.join(sep))
}

/// Paths for javac `@argfile`: strip `\\?\`, use `/` (Windows javac accepts these).
fn path_for_javac_arg(path: &Path) -> String {
    normalize_path(&path.to_string_lossy()).replace('\\', "/")
}

/// Classpath string for javac argfile: normalize each entry, join with OS sep, then `/`.
fn classpath_for_javac_arg(cp: &str) -> String {
    let sep = if cfg!(windows) { ';' } else { ':' };
    cp.split(sep)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| normalize_path(p).replace('\\', "/"))
        .collect::<Vec<_>>()
        .join(if cfg!(windows) { ";" } else { ":" })
}

/// IDEA-like: javac only the saved `.java` into `target/classes`.
fn javac_single_java_file(
    env: &RuntimeEnv,
    workspace: &Path,
    module_dir: &Path,
    java_file: &Path,
    unit_id: &str,
    app: &AppHandle,
    log: &dyn Fn(&str, String),
) -> Result<(), String> {
    let module_dir = PathBuf::from(normalize_path(&module_dir.to_string_lossy()));
    let java_file = PathBuf::from(normalize_path(&java_file.to_string_lossy()));
    let classes = module_dir.join("target").join("classes");
    if !classes.is_dir() {
        return Err(format!("missing {}", classes.display()));
    }
    let cp = ensure_compile_classpath(env, workspace, &module_dir, unit_id, app, log)?;
    let javac = resolve_javac_bin(env);
    // Use @argfile to avoid Windows CreateProcess / command-line length limits.
    let argfile = module_dir.join(".launcher-javac.args");
    let lines: Vec<String> = vec![
        "-encoding".into(),
        "UTF-8".into(),
        "-source".into(),
        "1.8".into(),
        "-target".into(),
        "1.8".into(),
        "-cp".into(),
        classpath_for_javac_arg(&cp),
        "-d".into(),
        path_for_javac_arg(&classes),
        path_for_javac_arg(&java_file),
    ];
    let content = lines.join("\n");
    fs::write(&argfile, format!("{content}\n")).map_err(|e| format!("write javac argfile: {e}"))?;

    log(
        "stdout",
        format!(
            "[launcher] hot-reload javac {}",
            java_file
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("*.java")
        ),
    );

    let at_arg = format!("@{}", path_for_javac_arg(&argfile));
    let ok = run_command_streaming(env, &module_dir, &javac, &[at_arg], unit_id, app)?;
    let _ = fs::remove_file(&argfile);
    if !ok {
        return Err("javac failed".into());
    }
    Ok(())
}

fn ensure_devtools_on_classpath(
    env: &RuntimeEnv,
    module_dir: &Path,
    classpath: &str,
    log: &dyn Fn(&str, String),
) -> Result<String, String> {
    let sep = if cfg!(windows) { ";" } else { ":" };
    if classpath.to_lowercase().contains("spring-boot-devtools") {
        log(
            "stdout",
            "[launcher] hot-reload: spring-boot-devtools already on classpath".into(),
        );
        return Ok(classpath.to_string());
    }
    let ver = extract_spring_boot_version(classpath)
        .unwrap_or_else(|| "2.1.12.RELEASE".to_string());
    let jar = effective_maven_local_repo(env)
        .join("org")
        .join("springframework")
        .join("boot")
        .join("spring-boot-devtools")
        .join(&ver)
        .join(format!("spring-boot-devtools-{ver}.jar"));

    if !jar.is_file() {
        log(
            "stdout",
            format!(
                "[launcher] hot-reload: fetching spring-boot-devtools:{ver} into local repo…"
            ),
        );
        fetch_devtools_jar(env, module_dir, &ver)?;
    }

    if !jar.is_file() {
        return Err(format!(
            "devtools jar not found at {} (check Maven repo / network)",
            jar.display()
        ));
    }
    log(
        "stdout",
        format!("[launcher] hot-reload: append {}", jar.display()),
    );
    Ok(format!("{classpath}{sep}{}", jar.display()))
}

fn fetch_devtools_jar(env: &RuntimeEnv, module_dir: &Path, ver: &str) -> Result<(), String> {
    let mvn = resolve_mvn_bin(env);
    let artifact = format!("org.springframework.boot:spring-boot-devtools:{ver}");
    let args = with_maven_settings_args(
        env,
        vec![
            "org.apache.maven.plugins:maven-dependency-plugin:3.6.1:get".into(),
            format!("-Dartifact={artifact}"),
        ],
    )?;
    let mut cmd = command_for(&mvn, &args);
    cmd.current_dir(module_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_env(&mut cmd, env);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("devtools dependency:get: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "devtools dependency:get failed: {} {}",
            stderr.trim(),
            stdout.trim()
        ));
    }
    Ok(())
}

fn prepare_and_run_vue(
    mgr: &Arc<ProcessManager>,
    app: &AppHandle,
    env: &RuntimeEnv,
    unit: &Launchable,
    log: &dyn Fn(&str, String),
) -> Result<(), String> {
    let module_path = normalize_path(&unit.path);
    let pm = unit.package_manager.as_deref().unwrap_or("npm");
    let script = unit.start_script.as_deref().unwrap_or("dev");
    let inv = resolve_pm_invocation(env, pm)?;
    // Keep console close to IDEA: only process output + npm lifecycle, no path dump.

    // Ensure deps if node_modules missing
    if !Path::new(&module_path).join("node_modules").is_dir() {
        log(
            "stdout",
            format!("[launcher] {pm} install (missing node_modules)"),
        );
        let mut install_args = inv.leading_args.clone();
        install_args.push("install".into());
        let ok = run_command_streaming(
            env,
            Path::new(&module_path),
            &inv.program,
            &install_args,
            &unit.id,
            app,
        )?;
        if !ok {
            return Err(format!("{pm} install failed"));
        }
    }

    clear_marker(mgr, &unit.id);
    let mut args = inv.leading_args.clone();
    args.push("run".to_string());
    args.push(script.to_string());
    // Embedded PTY → in-app xterm (not a popup console).
    spawn_embedded_pty(
        mgr,
        app,
        env,
        &unit.id,
        Path::new(&module_path),
        &inv.program,
        &args,
    )
}

fn clear_marker(mgr: &ProcessManager, unit_id: &str) {
    let mut guard = mgr.inner.lock().unwrap();
    if let Some(mp) = guard.remove(unit_id) {
        match mp {
            ManagedProcess::Piped { mut child } => {
                let pid = child.id();
                let _ = kill_process_tree(pid);
                let _ = child.try_wait();
            }
            ManagedProcess::Pty(session) => session.kill_tree(),
        }
    }
}

fn finish_failed(mgr: &ProcessManager, app: &AppHandle, unit_id: &str) {
    clear_marker(mgr, unit_id);
    let _ = app.emit(
        "module-status",
        StatusEvent {
            unit_id: unit_id.to_string(),
            status: ModuleStatus::Failed,
            pid: None,
            message: Some("start failed".into()),
        },
    );
}

fn spawn_marker(env: &RuntimeEnv) -> Result<Child, String> {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "3600", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_env(&mut cmd, env);
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd.spawn().map_err(|e| format!("marker process: {e}"))
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sleep");
        cmd.arg("3600")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_env(&mut cmd, env);
        cmd.spawn().map_err(|e| format!("marker process: {e}"))
    }
}

fn run_mvn_streaming(
    env: &RuntimeEnv,
    cwd: &Path,
    args: Vec<String>,
    unit_id: &str,
    app: &AppHandle,
) -> Result<bool, String> {
    let mvn = resolve_mvn_bin(env);
    // settings (-s) injected once inside run_command_streaming
    run_command_streaming(env, cwd, &mvn, &args, unit_id, app)
}

fn run_command_streaming(
    env: &RuntimeEnv,
    cwd: &Path,
    program: &Path,
    args: &[String],
    unit_id: &str,
    app: &AppHandle,
) -> Result<bool, String> {
    // If invoking Maven, inject -s settings.xml for repo overrides
    let effective_args = if program
        .file_name()
        .and_then(|s| s.to_str())
        .map(|n| n.to_lowercase().starts_with("mvn"))
        .unwrap_or(false)
    {
        with_maven_settings_args(env, args.to_vec())?
    } else {
        args.to_vec()
    };

    let mut cmd = command_for(program, &effective_args);
    cmd.current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_env(&mut cmd, env);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run {}: {e}", program.display()))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let app_out = app.clone();
    let id_out = unit_id.to_string();
    let t_out = std::thread::spawn(move || {
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().flatten() {
                let _ = app_out.emit(
                    "module-log",
                    LogEvent {
                        unit_id: id_out.clone(),
                        stream: "stdout".into(),
                        line,
                    },
                );
            }
        }
    });
    let app_err = app.clone();
    let id_err = unit_id.to_string();
    let t_err = std::thread::spawn(move || {
        if let Some(err) = stderr {
            for line in BufReader::new(err).lines().flatten() {
                let _ = app_err.emit(
                    "module-log",
                    LogEvent {
                        unit_id: id_err.clone(),
                        stream: "stderr".into(),
                        line,
                    },
                );
            }
        }
    });

    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    let _ = t_out.join();
    let _ = t_err.join();
    Ok(status.success())
}

fn spawn_long_running(
    mgr: &Arc<ProcessManager>,
    app: &AppHandle,
    env: &RuntimeEnv,
    unit_id: &str,
    cwd: &Path,
    program: &Path,
    args: &[String],
) -> Result<(), String> {
    let effective_args = if program
        .file_name()
        .and_then(|s| s.to_str())
        .map(|n| n.to_lowercase().starts_with("mvn"))
        .unwrap_or(false)
    {
        with_maven_settings_args(env, args.to_vec())?
    } else {
        args.to_vec()
    };

    let mut cmd = command_for(program, &effective_args);
    cmd.current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_env(&mut cmd, env);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", program.display()))?;
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    {
        let mut guard = mgr.inner.lock().unwrap();
        guard.insert(unit_id.to_string(), ManagedProcess::Piped { child });
    }

    let _ = app.emit(
        "module-status",
        StatusEvent {
            unit_id: unit_id.to_string(),
            status: ModuleStatus::Running,
            pid: Some(pid),
            message: None,
        },
    );

    if let Some(out) = stdout {
        let app2 = app.clone();
        let id2 = unit_id.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().flatten() {
                let _ = app2.emit(
                    "module-log",
                    LogEvent {
                        unit_id: id2.clone(),
                        stream: "stdout".into(),
                        line,
                    },
                );
            }
        });
    }
    if let Some(err) = stderr {
        let app2 = app.clone();
        let id2 = unit_id.to_string();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().flatten() {
                let _ = app2.emit(
                    "module-log",
                    LogEvent {
                        unit_id: id2.clone(),
                        stream: "stderr".into(),
                        line,
                    },
                );
            }
        });
    }

    watch_child_exit(mgr, app, unit_id);
    Ok(())
}

/// Vue/npm in an embedded PTY; bytes stream to frontend xterm via `module-term`.
fn spawn_embedded_pty(
    mgr: &Arc<ProcessManager>,
    app: &AppHandle,
    env: &RuntimeEnv,
    unit_id: &str,
    cwd: &Path,
    program: &Path,
    args: &[String],
) -> Result<(), String> {
    let session = crate::pty_session::PtySession::spawn(
        app.clone(),
        unit_id.to_string(),
        env,
        cwd,
        program,
        args,
        120,
        32,
    )?;
    let pid = session.pid;

    {
        let mut guard = mgr.inner.lock().unwrap();
        guard.insert(unit_id.to_string(), ManagedProcess::Pty(session));
    }

    let _ = app.emit(
        "module-status",
        StatusEvent {
            unit_id: unit_id.to_string(),
            status: ModuleStatus::Running,
            pid: if pid == 0 { None } else { Some(pid) },
            message: Some("pty".into()),
        },
    );

    watch_child_exit(mgr, app, unit_id);
    Ok(())
}

fn watch_child_exit(mgr: &Arc<ProcessManager>, app: &AppHandle, unit_id: &str) {
    let mgr2 = Arc::clone(mgr);
    let app2 = app.clone();
    let id2 = unit_id.to_string();
    std::thread::spawn(move || {
        let success = loop {
            let polled = {
                let mut guard = mgr2.inner.lock().unwrap();
                match guard.get_mut(&id2) {
                    Some(ManagedProcess::Piped { child }) => {
                        child.try_wait().ok().map(|o| o.map(|s| s.success()))
                    }
                    Some(ManagedProcess::Pty(s)) => {
                        s.try_wait().ok().map(|o| o.map(|st| st.success()))
                    }
                    None => return,
                }
            };
            match polled {
                Some(Some(ok)) => break ok,
                Some(None) => std::thread::sleep(std::time::Duration::from_millis(200)),
                None => return,
            }
        };
        {
            let mut guard = mgr2.inner.lock().unwrap();
            guard.remove(&id2);
        }
        let _ = app2.emit(
            "module-status",
            StatusEvent {
                unit_id: id2,
                status: if success {
                    ModuleStatus::Stopped
                } else {
                    ModuleStatus::Failed
                },
                pid: None,
                message: None,
            },
        );
    });
}

pub(crate) fn kill_process_tree(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        let output = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output()
            .map_err(|e| format!("taskkill failed: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            if !err.to_lowercase().contains("not found") {
                return Err(format!("taskkill: {err}"));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .output();
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_process_tree_stops_child() {
        #[cfg(windows)]
        let mut child = Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ping");
        #[cfg(not(windows))]
        let mut child = Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");

        let pid = child.id();
        kill_process_tree(pid).expect("kill tree");
        let _ = child.try_wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(child.try_wait().expect("try_wait").is_some());
    }
}
