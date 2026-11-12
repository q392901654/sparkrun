use crate::model::RuntimeEnv;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn resolve_java_bin(env: &RuntimeEnv) -> PathBuf {
    if !env.java_home.trim().is_empty() {
        let home = Path::new(env.java_home.trim());
        #[cfg(windows)]
        {
            let candidate = home.join("bin").join("java.exe");
            if candidate.exists() {
                return candidate;
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = home.join("bin").join("java");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from(if cfg!(windows) { "java.exe" } else { "java" })
}

/// `javac` next to `java` from JAVA_HOME (for single-file incremental compile).
pub fn resolve_javac_bin(env: &RuntimeEnv) -> PathBuf {
    if !env.java_home.trim().is_empty() {
        let home = Path::new(env.java_home.trim());
        #[cfg(windows)]
        {
            let candidate = home.join("bin").join("javac.exe");
            if candidate.exists() {
                return candidate;
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = home.join("bin").join("javac");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    // Same directory as resolved java, if possible
    let java = resolve_java_bin(env);
    if let Some(dir) = java.parent() {
        #[cfg(windows)]
        {
            let candidate = dir.join("javac.exe");
            if candidate.exists() {
                return candidate;
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join("javac");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from(if cfg!(windows) { "javac.exe" } else { "javac" })
}

pub fn resolve_mvn_bin(env: &RuntimeEnv) -> PathBuf {
    if !env.maven_home.trim().is_empty() {
        let home = Path::new(env.maven_home.trim());
        #[cfg(windows)]
        {
            let cmd = home.join("bin").join("mvn.cmd");
            if cmd.exists() {
                return cmd;
            }
            let bat = home.join("bin").join("mvn.bat");
            if bat.exists() {
                return bat;
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = home.join("bin").join("mvn");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from(if cfg!(windows) { "mvn.cmd" } else { "mvn" })
}

/// Candidate executable names for a Node toolchain binary (`node` / `npm` / `pnpm` / `yarn`).
fn node_tool_names(tool: &str) -> Vec<String> {
    if cfg!(windows) {
        if tool == "node" {
            vec!["node.exe".into()]
        } else {
            // Prefer `.exe` — CreateProcess can run it; `.cmd` needs `cmd.exe /c`.
            vec![format!("{tool}.exe"), format!("{tool}.cmd")]
        }
    } else {
        vec![tool.to_string()]
    }
}

fn first_existing(dir: &Path, names: &[String]) -> Option<PathBuf> {
    for name in names {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        let in_bin = dir.join("bin").join(name);
        if in_bin.is_file() {
            return Some(in_bin);
        }
    }
    None
}

fn find_on_path(names: &[String]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Extra dirs when GUI PATH lacks shell-only shims (nvmd / nvm / official installer).
fn common_node_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        dirs.push(PathBuf::from(pf).join("nodejs"));
    }
    if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
        dirs.push(PathBuf::from(pf86).join("nodejs"));
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".nvmd").join("bin"));
        dirs.push(home.join("AppData").join("Roaming").join("nvm"));
        dirs.push(home.join(".fnm").join("current"));
        dirs.push(home.join(".local").join("share").join("fnm").join("current"));
    }
    if let Ok(nvm_symlink) = std::env::var("NVM_SYMLINK") {
        dirs.push(PathBuf::from(nvm_symlink));
    }
    if let Ok(nvm_home) = std::env::var("NVM_HOME") {
        dirs.push(PathBuf::from(nvm_home));
    }
    dirs
}

pub fn resolve_node_bin(env: &RuntimeEnv, tool: &str) -> PathBuf {
    let names = node_tool_names(tool);
    if !env.node_home.trim().is_empty() {
        if let Some(p) = first_existing(Path::new(env.node_home.trim()), &names) {
            return p;
        }
    }
    if let Some(p) = find_on_path(&names) {
        return p;
    }
    for dir in common_node_dirs() {
        if let Some(p) = first_existing(&dir, &names) {
            return p;
        }
    }
    PathBuf::from(names.into_iter().next().unwrap_or_else(|| tool.to_string()))
}

fn npm_cli_js_near(node_or_home: &Path) -> Option<PathBuf> {
    let dir = if node_or_home.is_file() {
        node_or_home.parent()?.to_path_buf()
    } else {
        node_or_home.to_path_buf()
    };
    let cli = dir
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js");
    if cli.is_file() {
        Some(cli)
    } else {
        None
    }
}

/// How to invoke a package manager without Windows `npm.cmd` global-prefix hijack.
///
/// Official `npm.cmd` runs `npm prefix -g` and may switch to another Node's npm
/// (e.g. NODE 12 + npm from D:\\npmversion\\22.x → `Cannot find module 'node:path'`).
/// We call `node.exe …/npm-cli.js` from the same install instead.
#[derive(Debug, Clone)]
pub struct PmInvocation {
    pub program: PathBuf,
    /// Args inserted before user args (e.g. path to npm-cli.js).
    pub leading_args: Vec<String>,
}

pub fn resolve_pm_invocation(env: &RuntimeEnv, pm: &str) -> Result<PmInvocation, String> {
    let pm = if pm.trim().is_empty() { "npm" } else { pm };

    if pm == "npm" {
        let node = resolve_node_bin(env, "node");
        if node.is_file() {
            if let Some(cli) = npm_cli_js_near(&node) {
                return Ok(PmInvocation {
                    program: node,
                    leading_args: vec![cli.to_string_lossy().to_string()],
                });
            }
        }
        // Fall through to npm.exe / npm.cmd
    }

    // pnpm / yarn / npm fallback: prefer real binary next to node when possible
    if let Some(home) = {
        let n = resolve_node_bin(env, "node");
        n.parent().map(|p| p.to_path_buf())
    } {
        if let Some(p) = first_existing(&home, &node_tool_names(pm)) {
            if p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false)
                || !cfg!(windows)
            {
                return Ok(PmInvocation {
                    program: p,
                    leading_args: Vec::new(),
                });
            }
        }
    }

    let bin = resolve_node_bin(env, pm);
    if !bin.is_absolute() || !bin.is_file() {
        return Err(format!(
            "找不到 {pm}（解析为 {}）。请在该 Vue 模块设置 NODE_HOME，或在全局工具链配置 Node 安装目录。",
            bin.display()
        ));
    }
    Ok(PmInvocation {
        program: bin,
        leading_args: Vec::new(),
    })
}

/// Build a `Command` that can run `.exe` or Windows `.cmd`/`.bat` scripts.
pub fn command_for(program: &Path, args: &[String]) -> Command {
    #[cfg(windows)]
    {
        let is_script = program
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let e = e.to_ascii_lowercase();
                e == "cmd" || e == "bat"
            })
            .unwrap_or(false);
        if is_script {
            // CreateProcess cannot run batch files directly.
            let mut cmd = Command::new("cmd.exe");
            cmd.arg("/D").arg("/S").arg("/C");
            cmd.arg(program.as_os_str());
            for a in args {
                cmd.arg(a);
            }
            return cmd;
        }
    }
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd
}

fn path_dir_looks_like_other_node(dir: &Path, keep: &Path) -> bool {
    if dir == keep || dir.starts_with(keep) || keep.starts_with(dir) {
        return false;
    }
    let name = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name == "nodejs" || name == "node" || name == "bin" {
        if dir.join("node.exe").is_file() || dir.join("node").is_file() {
            return true;
        }
        if dir.join("npm.cmd").is_file() || dir.join("npm").is_file() {
            return true;
        }
    }
    // nvmd / nvm version folders, custom installs like D:\npmversion\22.x
    if dir.join("node.exe").is_file() || dir.join("node").is_file() {
        return true;
    }
    false
}

pub fn build_path_env(env: &RuntimeEnv) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !env.java_home.trim().is_empty() {
        parts.push(
            Path::new(env.java_home.trim())
                .join("bin")
                .to_string_lossy()
                .to_string(),
        );
    }
    if !env.maven_home.trim().is_empty() {
        parts.push(
            Path::new(env.maven_home.trim())
                .join("bin")
                .to_string_lossy()
                .to_string(),
        );
    }
    let node_home = env.node_home.trim();
    let keep_node = if !node_home.is_empty() {
        let home = Path::new(node_home);
        parts.push(home.to_string_lossy().to_string());
        let bin = home.join("bin");
        if bin.is_dir() {
            parts.push(bin.to_string_lossy().to_string());
        }
        Some(home.to_path_buf())
    } else {
        None
    };
    if let Ok(existing) = std::env::var("PATH") {
        let sep = if cfg!(windows) { ';' } else { ':' };
        for segment in existing.split(sep) {
            if segment.is_empty() {
                continue;
            }
            if let Some(ref keep) = keep_node {
                if path_dir_looks_like_other_node(Path::new(segment), keep) {
                    continue;
                }
            }
            parts.push(segment.to_string());
        }
    }
    let sep = if cfg!(windows) { ";" } else { ":" };
    parts.join(sep)
}

pub fn apply_env(cmd: &mut Command, env: &RuntimeEnv) {
    if !env.java_home.trim().is_empty() {
        cmd.env("JAVA_HOME", env.java_home.trim());
    }
    if !env.maven_home.trim().is_empty() {
        cmd.env("MAVEN_HOME", env.maven_home.trim());
        cmd.env("M2_HOME", env.maven_home.trim());
    }
    if !env.node_home.trim().is_empty() {
        cmd.env("NODE_HOME", env.node_home.trim());
        // Prevent npm.cmd / npm from redirecting to another install's global prefix.
        cmd.env_remove("npm_config_prefix");
        cmd.env_remove("NPM_CONFIG_PREFIX");
        cmd.env_remove("PREFIX");
    }
    // Match typical IDEA/npm Run console verbosity (lifecycle + notices, not silly).
    if std::env::var_os("npm_config_loglevel").is_none() {
        cmd.env("npm_config_loglevel", "notice");
    }
    // Force chalk/ora/npm colors even without a real PTY (IDEA Terminal look).
    cmd.env("FORCE_COLOR", "1");
    cmd.env("npm_config_color", "always");
    cmd.env("COLORTERM", "truecolor");
    if std::env::var_os("TERM").is_none() {
        cmd.env("TERM", "xterm-256color");
    }
    cmd.env("PATH", build_path_env(env));
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Local Maven repository directory used for installs / probes (aligned with `-s` settings).
pub fn effective_maven_local_repo(env: &RuntimeEnv) -> PathBuf {
    let configured = env.maven_local_repo.trim();
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".m2").join("repository")
}

/// Write a temporary settings.xml when repo URL and/or local repo path is configured.
/// Returns path to pass as `mvn -s <path>`.
pub fn ensure_maven_settings_file(env: &RuntimeEnv) -> Result<Option<PathBuf>, String> {
    let repo_url = env.maven_repo_url.trim();
    let local_repo = env.maven_local_repo.trim();
    if repo_url.is_empty() && local_repo.is_empty() {
        return Ok(None);
    }

    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<settings xmlns="http://maven.apache.org/SETTINGS/1.2.0"
          xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
          xsi:schemaLocation="http://maven.apache.org/SETTINGS/1.2.0 https://maven.apache.org/xsd/settings-1.2.0.xsd">
"#,
    );
    if !local_repo.is_empty() {
        xml.push_str(&format!(
            "  <localRepository>{}</localRepository>\n",
            xml_escape(local_repo)
        ));
    }
    if !repo_url.is_empty() {
        xml.push_str(&format!(
            r#"  <mirrors>
    <mirror>
      <id>launcher-mirror</id>
      <name>SparkRun Mirror</name>
      <url>{}</url>
      <mirrorOf>*</mirrorOf>
    </mirror>
  </mirrors>
"#,
            xml_escape(repo_url)
        ));
    }
    xml.push_str("</settings>\n");

    let dir = std::env::temp_dir().join("sparkrun");
    fs::create_dir_all(&dir).map_err(|e| format!("create temp dir: {e}"))?;
    // Stable name from content so we rewrite when config changes
    let hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        repo_url.hash(&mut h);
        local_repo.hash(&mut h);
        format!("{:x}", h.finish())
    };
    let path = dir.join(format!("settings-{hash}.xml"));
    fs::write(&path, xml).map_err(|e| format!("write maven settings: {e}"))?;
    Ok(Some(path))
}

/// Prepend `-s <settings.xml>` when custom Maven repository is configured.
pub fn with_maven_settings_args(env: &RuntimeEnv, args: Vec<String>) -> Result<Vec<String>, String> {
    match ensure_maven_settings_file(env)? {
        Some(path) => {
            let mut out = vec!["-s".into(), path.to_string_lossy().to_string()];
            out.extend(args);
            Ok(out)
        }
        None => Ok(args),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainInfo {
    pub java_ok: bool,
    pub java_version: String,
    pub maven_ok: bool,
    pub maven_version: String,
    pub node_ok: bool,
    pub node_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainHomeHit {
    pub path: String,
    pub label: String,
    /// env | path | known
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverToolchainHomes {
    pub java_homes: Vec<ToolchainHomeHit>,
    pub maven_homes: Vec<ToolchainHomeHit>,
    pub node_homes: Vec<ToolchainHomeHit>,
}

fn push_unique_home(out: &mut Vec<ToolchainHomeHit>, path: PathBuf, label: String, source: &str) {
    let Ok(canon) = path.canonicalize() else {
        if !path.is_dir() {
            return;
        }
        let p = path.to_string_lossy().replace('/', "\\");
        if out.iter().any(|h| h.path.eq_ignore_ascii_case(&p)) {
            return;
        }
        out.push(ToolchainHomeHit {
            path: p,
            label,
            source: source.into(),
        });
        return;
    };
    let p = canon.to_string_lossy().replace('/', "\\");
    if out.iter().any(|h| h.path.eq_ignore_ascii_case(&p)) {
        return;
    }
    out.push(ToolchainHomeHit {
        path: p,
        label,
        source: source.into(),
    });
}

fn home_from_bin_exe(exe: &Path) -> Option<PathBuf> {
    let bin = exe.parent()?;
    let name = bin
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name == "bin" {
        bin.parent().map(|p| p.to_path_buf())
    } else {
        // node often lives directly in install root (…/nodejs/node.exe)
        Some(bin.to_path_buf())
    }
}

fn looks_like_java_home(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        dir.join("bin").join("java.exe").is_file()
    }
    #[cfg(not(windows))]
    {
        dir.join("bin").join("java").is_file()
    }
}

fn looks_like_maven_home(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        dir.join("bin").join("mvn.cmd").is_file()
            || dir.join("bin").join("mvn.bat").is_file()
            || dir.join("bin").join("mvn").is_file()
    }
    #[cfg(not(windows))]
    {
        dir.join("bin").join("mvn").is_file()
    }
}

fn looks_like_node_home(dir: &Path) -> bool {
    #[cfg(windows)]
    {
        dir.join("node.exe").is_file() || dir.join("bin").join("node.exe").is_file()
    }
    #[cfg(not(windows))]
    {
        dir.join("bin").join("node").is_file() || dir.join("node").is_file()
    }
}

/// Resolve executable via PATH without scanning the disk (`where` / `which`).
fn which_on_path(name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let output = Command::new("where.exe").arg(name).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let first = text.lines().map(str::trim).find(|l| !l.is_empty())?;
        let p = PathBuf::from(first);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        let output = Command::new("which").arg(name).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let first = text.lines().map(str::trim).find(|l| !l.is_empty())?;
        let p = PathBuf::from(first);
        if p.is_file() {
            Some(p)
        } else {
            None
        }
    }
}

fn read_dir_filtered(parent: &Path, mut keep: impl FnMut(&str) -> bool) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let Ok(ft) = ent.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().to_string();
        if keep(&name) {
            out.push(ent.path());
        }
    }
    out
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Fast discovery: env + PATH (`where`/`which`) + shallow known install folders only.
/// Never walks the whole disk.
pub fn discover_toolchain_homes() -> DiscoverToolchainHomes {
    let mut java = Vec::new();
    let mut maven = Vec::new();
    let mut node = Vec::new();

    // --- env ---
    if let Some(p) = env_path("JAVA_HOME") {
        if looks_like_java_home(&p) {
            push_unique_home(&mut java, p, "JAVA_HOME".into(), "env");
        }
    }
    for key in ["MAVEN_HOME", "M2_HOME"] {
        if let Some(p) = env_path(key) {
            if looks_like_maven_home(&p) {
                push_unique_home(&mut maven, p, key.into(), "env");
            }
        }
    }
    if let Some(p) = env_path("NODE_HOME") {
        if looks_like_node_home(&p) {
            push_unique_home(&mut node, p, "NODE_HOME".into(), "env");
        }
    }

    // --- PATH ---
    for exe_name in ["java.exe", "java"] {
        if let Some(exe) = which_on_path(exe_name) {
            if let Some(home) = home_from_bin_exe(&exe) {
                if looks_like_java_home(&home) {
                    push_unique_home(&mut java, home, "PATH".into(), "path");
                }
            }
            break;
        }
    }
    for exe_name in ["mvn.cmd", "mvn"] {
        if let Some(exe) = which_on_path(exe_name) {
            if let Some(home) = home_from_bin_exe(&exe) {
                if looks_like_maven_home(&home) {
                    push_unique_home(&mut maven, home, "PATH".into(), "path");
                }
            }
            break;
        }
    }
    for exe_name in ["node.exe", "node"] {
        if let Some(exe) = which_on_path(exe_name) {
            if let Some(home) = home_from_bin_exe(&exe) {
                if looks_like_node_home(&home) {
                    push_unique_home(&mut node, home, "PATH".into(), "path");
                }
            }
            break;
        }
    }

    // --- known shallow roots (Windows-oriented + portable) ---
    let mut roots: Vec<PathBuf> = Vec::new();
    for key in [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "HOME",
    ] {
        if let Some(p) = env_path(key) {
            roots.push(p);
        }
    }
    // Extra common drive roots — only one-level name filter, not recursive
    for drive in ["C:\\", "D:\\", "E:\\"] {
        let p = PathBuf::from(drive);
        if p.is_dir() {
            roots.push(p);
        }
    }

    for root in &roots {
        let root_name = root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        // Direct Program Files vendors
        for vendor in [
            "Java",
            "Eclipse Adoptium",
            "Microsoft",
            "Amazon Corretto",
            "Zulu",
            "BellSoft",
            "Temurin",
            "JavaSoft",
            "AdoptOpenJDK",
        ] {
            let vendor_dir = root.join(vendor);
            if vendor_dir.is_dir() {
                for child in read_dir_filtered(&vendor_dir, |_| true) {
                    if looks_like_java_home(&child) {
                        let label = child
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("JDK")
                            .to_string();
                        push_unique_home(&mut java, child, label, "known");
                    }
                }
            }
        }

        // JetBrains downloaded JDKs
        let jdks = root.join(".jdks");
        if jdks.is_dir() {
            for child in read_dir_filtered(&jdks, |_| true) {
                if looks_like_java_home(&child) {
                    let label = child
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("JDK")
                        .to_string();
                    push_unique_home(&mut java, child, label, "known");
                }
            }
        }

        // Drive root: jdk* / jre* / apache-maven* / maven* / node* (one level only)
        if root_name.is_empty() || root_name.len() <= 2 {
            for child in read_dir_filtered(root, |n| {
                let l = n.to_ascii_lowercase();
                l.starts_with("jdk")
                    || l.starts_with("jre")
                    || l.starts_with("java")
                    || l.contains("temurin")
                    || l.contains("corretto")
                    || l.contains("zulu")
            }) {
                if looks_like_java_home(&child) {
                    let label = child
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("JDK")
                        .to_string();
                    push_unique_home(&mut java, child, label, "known");
                }
            }
            for child in read_dir_filtered(root, |n| {
                let l = n.to_ascii_lowercase();
                l.starts_with("apache-maven") || l.starts_with("maven")
            }) {
                if looks_like_maven_home(&child) {
                    let label = child
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Maven")
                        .to_string();
                    push_unique_home(&mut maven, child, label, "known");
                }
            }
        }

        // Node: Program Files\nodejs
        let nodejs = root.join("nodejs");
        if looks_like_node_home(&nodejs) {
            push_unique_home(&mut node, nodejs, "nodejs".into(), "known");
        }

        // nvm-windows: %APPDATA%\nvm\<version> or settings in root
        let nvm = root.join("nvm");
        if nvm.is_dir() {
            for child in read_dir_filtered(&nvm, |n| {
                let l = n.to_ascii_lowercase();
                l.starts_with('v') || l.chars().next().is_some_and(|c| c.is_ascii_digit())
            }) {
                if looks_like_node_home(&child) {
                    let label = child
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("node")
                        .to_string();
                    push_unique_home(&mut node, child, format!("nvm {label}"), "known");
                }
            }
        }

        // ~/.nvm/versions/node/*
        let nvm_versions = root.join(".nvm").join("versions").join("node");
        if nvm_versions.is_dir() {
            for child in read_dir_filtered(&nvm_versions, |_| true) {
                if looks_like_node_home(&child) {
                    let label = child
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("node")
                        .to_string();
                    push_unique_home(&mut node, child, format!("nvm {label}"), "known");
                }
            }
        }

        // Apache under Program Files
        let apache = root.join("Apache");
        if apache.is_dir() {
            for child in read_dir_filtered(&apache, |n| {
                let l = n.to_ascii_lowercase();
                l.contains("maven")
            }) {
                if looks_like_maven_home(&child) {
                    let label = child
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Maven")
                        .to_string();
                    push_unique_home(&mut maven, child, label, "known");
                }
            }
        }
    }

    // Soft cap — keep response small
    java.truncate(24);
    maven.truncate(16);
    node.truncate(24);

    DiscoverToolchainHomes {
        java_homes: java,
        maven_homes: maven,
        node_homes: node,
    }
}

pub fn detect_toolchain(env: &RuntimeEnv) -> ToolchainInfo {
    let java = resolve_java_bin(env);
    let mut java_cmd = Command::new(&java);
    java_cmd.arg("-version");
    apply_env(&mut java_cmd, env);
    let (java_ok, java_version) = match java_cmd.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stderr).to_string();
            if text.trim().is_empty() {
                text = String::from_utf8_lossy(&out.stdout).to_string();
            }
            let summary = text.lines().next().unwrap_or("").trim().to_string();
            (out.status.success() || !summary.is_empty(), summary)
        }
        Err(e) => (false, format!("failed to run java: {e}")),
    };

    let mvn = resolve_mvn_bin(env);
    let mut mvn_cmd = Command::new(&mvn);
    if let Ok(Some(settings)) = ensure_maven_settings_file(env) {
        mvn_cmd.arg("-s").arg(&settings);
    }
    mvn_cmd.arg("-v");
    apply_env(&mut mvn_cmd, env);
    let (maven_ok, maven_version) = match mvn_cmd.output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            let summary = text
                .lines()
                .find(|l| l.to_lowercase().contains("apache maven"))
                .or_else(|| text.lines().next())
                .unwrap_or("")
                .trim()
                .to_string();
            let err = String::from_utf8_lossy(&out.stderr).to_string();
            if summary.is_empty() && !err.trim().is_empty() {
                (false, err.lines().next().unwrap_or("").trim().to_string())
            } else {
                (out.status.success(), summary)
            }
        }
        Err(e) => (false, format!("failed to run mvn: {e}")),
    };

    let node = resolve_node_bin(env, "node");
    let mut node_cmd = Command::new(&node);
    node_cmd.arg("-v");
    apply_env(&mut node_cmd, env);
    let (node_ok, node_version) = match node_cmd.output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            (out.status.success(), text)
        }
        Err(e) => (false, format!("failed to run node: {e}")),
    };

    ToolchainInfo {
        java_ok,
        java_version,
        maven_ok,
        maven_version,
        node_ok,
        node_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RuntimeEnv;

    #[test]
    fn detect_toolchain_runs() {
        let env = RuntimeEnv {
            java_home: String::new(),
            maven_home: String::new(),
            node_home: String::new(),
            maven_repo_url: String::new(),
            maven_local_repo: String::new(),
            workspace_path: String::new(),
        };
        let info = detect_toolchain(&env);
        assert!(!info.java_version.is_empty() || !info.java_ok);
    }

    #[test]
    fn node_tool_names_prefer_exe_on_windows() {
        #[cfg(windows)]
        {
            assert_eq!(node_tool_names("node"), vec!["node.exe".to_string()]);
            assert_eq!(
                node_tool_names("npm"),
                vec!["npm.exe".to_string(), "npm.cmd".to_string()]
            );
        }
        #[cfg(not(windows))]
        {
            assert_eq!(node_tool_names("node"), vec!["node".to_string()]);
            assert_eq!(node_tool_names("npm"), vec!["npm".to_string()]);
        }
    }

    #[test]
    fn resolve_node_finds_nvmd_or_path() {
        let env = RuntimeEnv {
            java_home: String::new(),
            maven_home: String::new(),
            node_home: String::new(),
            maven_repo_url: String::new(),
            maven_local_repo: String::new(),
            workspace_path: String::new(),
        };
        let npm = resolve_node_bin(&env, "npm");
        // On this machine PATH/nvmd should yield an absolute file; otherwise bare fallback.
        if npm.is_absolute() {
            assert!(npm.is_file(), "resolved npm must exist: {}", npm.display());
        }
    }

    #[test]
    fn resolve_pm_prefers_node_plus_local_cli() {
        let nvm12 = PathBuf::from(r"C:\Users\39290\AppData\Roaming\nvm\v12.22.11");
        if !nvm12.join("node.exe").is_file() {
            return;
        }
        let env = RuntimeEnv {
            java_home: String::new(),
            maven_home: String::new(),
            node_home: nvm12.to_string_lossy().to_string(),
            maven_repo_url: String::new(),
            maven_local_repo: String::new(),
            workspace_path: String::new(),
        };
        let inv = resolve_pm_invocation(&env, "npm").expect("pm");
        assert_eq!(inv.program, nvm12.join("node.exe"));
        assert_eq!(
            inv.leading_args,
            vec![nvm12
                .join("node_modules")
                .join("npm")
                .join("bin")
                .join("npm-cli.js")
                .to_string_lossy()
                .to_string()]
        );
    }
}
