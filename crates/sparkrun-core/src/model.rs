use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Extensible project / module kinds. Add new variants as support grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectKind {
    SpringBoot,
    Vue,
}

impl ProjectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectKind::SpringBoot => "springBoot",
            ProjectKind::Vue => "vue",
        }
    }

}

/// A discoverable runnable unit under a project root.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launchable {
    /// Stable id: `{kind}:{normalizedPath}`
    pub id: String,
    pub kind: ProjectKind,
    pub name: String,
    pub path: String,
    pub relative_path: String,
    pub detail: Option<String>,
    /// Vue: npm|pnpm|yarn；Spring Boot 可空
    pub package_manager: Option<String>,
    /// Vue: script name (dev/serve/start)
    pub start_script: Option<String>,
    /// Spring Boot: detected config profiles. Sentinel `"application"` = default (no active profile).
    #[serde(default)]
    pub spring_profiles: Vec<String>,
    /// Spring Boot: selected profile for next start (UI may override via start_unit arg).
    #[serde(default = "default_spring_profile")]
    pub selected_spring_profile: String,
}

fn default_spring_profile() -> String {
    "application".to_string()
}

impl Launchable {
    pub fn make_id(kind: ProjectKind, path: &str) -> String {
        format!("{}:{}", kind.as_str(), path)
    }
}

/// Build-before-run strategy (IDEA-like).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum BuildBeforeRun {
    #[default]
    Smart,
    Always,
    Never,
}

/// Per-project configuration (overrides global defaults when non-empty).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root_path: String,
    #[serde(default)]
    pub java_home: String,
    #[serde(default)]
    pub maven_home: String,
    #[serde(default)]
    pub node_home: String,
    /// Remote Maven repository / mirror URL (e.g. Nexus). Empty = use Maven default.
    #[serde(default)]
    pub maven_repo_url: String,
    /// Local Maven repository directory. Empty = `~/.m2/repository`.
    #[serde(default)]
    pub maven_local_repo: String,
    /// 0 = use default (3)
    #[serde(default)]
    pub scan_depth: u32,
    #[serde(default = "default_true")]
    pub enable_spring_boot: bool,
    #[serde(default = "default_true")]
    pub enable_vue: bool,
    #[serde(default)]
    pub build_before_run: BuildBeforeRun,
    /// Watch + incremental compile on save (no JVM restart by itself).
    #[serde(default)]
    pub hot_reload: bool,
    /// Also inject DevTools and restart ApplicationContext after compile (slow; near full reboot).
    #[serde(default)]
    pub hot_reload_restart: bool,
}

fn default_true() -> bool {
    true
}

impl Project {
    pub fn new(name: String, root_path: String) -> Self {
        let id = format!(
            "proj_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        );
        Self {
            id,
            name,
            root_path,
            java_home: String::new(),
            maven_home: String::new(),
            node_home: String::new(),
            maven_repo_url: String::new(),
            maven_local_repo: String::new(),
            scan_depth: 0,
            enable_spring_boot: true,
            enable_vue: true,
            build_before_run: BuildBeforeRun::Smart,
            hot_reload: false,
            hot_reload_restart: false,
        }
    }

    pub fn effective_scan_depth(&self) -> usize {
        if self.scan_depth == 0 {
            3
        } else {
            self.scan_depth as usize
        }
    }

    pub fn enabled_kinds(&self) -> HashSet<ProjectKind> {
        let mut set = HashSet::new();
        if self.enable_spring_boot {
            set.insert(ProjectKind::SpringBoot);
        }
        if self.enable_vue {
            set.insert(ProjectKind::Vue);
        }
        set
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum UiTheme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum UiAccent {
    #[default]
    Blue,
    Teal,
    Green,
    Orange,
    Purple,
    Rose,
}

/// Appearance preferences for the launcher UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPreferences {
    #[serde(default)]
    pub theme: UiTheme,
    /// Base font size in px (12–20).
    #[serde(default = "default_font_size")]
    pub font_size: u32,
    #[serde(default)]
    pub accent: UiAccent,
    /// UI density: comfortable | compact
    #[serde(default = "default_density")]
    pub density: String,
}

fn default_font_size() -> u32 {
    14
}

fn default_density() -> String {
    "comfortable".into()
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            theme: UiTheme::System,
            font_size: 14,
            accent: UiAccent::Blue,
            density: "comfortable".into(),
        }
    }
}

/// App-wide defaults (toolchain + UI). Projects may override toolchain paths.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSettings {
    #[serde(default)]
    pub java_home: String,
    #[serde(default)]
    pub maven_home: String,
    #[serde(default)]
    pub node_home: String,
    #[serde(default)]
    pub maven_repo_url: String,
    #[serde(default)]
    pub maven_local_repo: String,
    #[serde(default)]
    pub active_project_id: Option<String>,
    /// Legacy field — migrated into a Project on load
    #[serde(default)]
    pub workspace_path: String,
    #[serde(default)]
    pub ui: UiPreferences,
}

/// Effective env after merging global + project.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEnv {
    pub java_home: String,
    pub maven_home: String,
    pub node_home: String,
    pub maven_repo_url: String,
    pub maven_local_repo: String,
    pub workspace_path: String,
}

impl RuntimeEnv {
    pub fn merge(global: &GlobalSettings, project: &Project) -> Self {
        Self {
            java_home: first_non_empty(&project.java_home, &global.java_home),
            maven_home: first_non_empty(&project.maven_home, &global.maven_home),
            node_home: first_non_empty(&project.node_home, &global.node_home),
            maven_repo_url: first_non_empty(&project.maven_repo_url, &global.maven_repo_url),
            maven_local_repo: first_non_empty(&project.maven_local_repo, &global.maven_local_repo),
            workspace_path: project.root_path.clone(),
        }
    }

    /// Per-unit NODE_HOME overrides project/global when non-empty.
    pub fn with_node_home_override(mut self, node_home: Option<&str>) -> Self {
        if let Some(h) = node_home.map(str::trim).filter(|s| !s.is_empty()) {
            self.node_home = h.to_string();
        }
        self
    }
}

fn first_non_empty(a: &str, b: &str) -> String {
    if !a.trim().is_empty() {
        a.trim().to_string()
    } else {
        b.trim().to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStore {
    #[serde(default)]
    pub projects: Vec<Project>,
}

/// Persisted scan result for a project (app_data_dir/scan-cache/{id}.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanCache {
    pub project_id: String,
    pub root_path: String,
    pub scanned_at: u64,
    #[serde(default)]
    pub units: Vec<Launchable>,
    #[serde(default)]
    pub active_unit_id: Option<String>,
    #[serde(default)]
    pub profile_by_unit: HashMap<String, String>,
    /// Per Vue (or other Node) unit: NODE_HOME override. Empty/missing = project/global.
    #[serde(default)]
    pub node_home_by_unit: HashMap<String, String>,
    /// Starred unit ids (UI pin-to-top); kept across rescan when unit still exists.
    #[serde(default)]
    pub starred_unit_ids: Vec<String>,
    /// Per-unit starred file paths for quick open (e.g. config files).
    #[serde(default)]
    pub starred_files_by_unit: HashMap<String, Vec<String>>,
}

/// Emitted during scan so the UI status bar can update without blocking.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub project_id: String,
    /// springBoot | vue | done | error
    pub phase: String,
    pub current: u32,
    pub total: Option<u32>,
    pub found: u32,
    pub message: String,
}
