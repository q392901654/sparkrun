//! Git CLI wrappers for SparkRun Git pane (lazygit-inspired).
//! Uses system `git` on PATH; no libgit2.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoSummary {
    pub is_repo: bool,
    pub toplevel: Option<String>,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub merge_in_progress: bool,
    pub rebase_in_progress: bool,
    pub conflict_count: u32,
    pub merging_into: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusEntry {
    pub path: String,
    pub orig_path: Option<String>,
    /// Index / worktree XY status (e.g. "M ", " M", "UU", "??")
    pub status: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub unmerged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitBranchInfo {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub author: String,
    pub relative_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStashEntry {
    pub index: u32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitConflictSides {
    pub path: String,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    pub worktree: Option<String>,
    pub has_stage1: bool,
    pub has_stage2: bool,
    pub has_stage3: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitOpResult {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    /// True when merge/pull left conflicts
    pub conflicts: bool,
    pub conflict_paths: Vec<String>,
}

fn git_cmd(repo: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "未找到 git：请安装 Git 并确保已加入 PATH".into()
            } else {
                format!("git 执行失败: {e}")
            }
        })
}

fn git_run(repo: &Path, args: &[&str]) -> Result<(String, String, i32), String> {
    let out = git_cmd(repo, args)?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim_end().to_string();
    let code = out.status.code().unwrap_or(-1);
    Ok((stdout, stderr, code))
}

fn git_ok(repo: &Path, args: &[&str]) -> Result<String, String> {
    let (stdout, stderr, code) = git_run(repo, args)?;
    if code != 0 {
        let msg = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("git {:?} 失败 (exit {code})", args)
        };
        return Err(msg);
    }
    Ok(stdout)
}

/// Resolve and canonicalize repo root; must be a git worktree.
fn resolve_repo(root_path: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(root_path);
    if !root.exists() {
        return Err(format!("路径不存在: {root_path}"));
    }
    let (stdout, stderr, code) = git_run(&root, &["rev-parse", "--show-toplevel"])?;
    if code != 0 {
        return Err(if !stderr.is_empty() {
            stderr
        } else {
            "当前目录不是 git 仓库".into()
        });
    }
    let toplevel = PathBuf::from(stdout.trim());
    Ok(toplevel)
}

fn resolve_rel_path(repo: &Path, path: &str) -> Result<(PathBuf, String), String> {
    let raw = PathBuf::from(path);
    for c in raw.components() {
        if matches!(c, Component::ParentDir) {
            return Err("非法路径".into());
        }
    }
    let abs = if raw.is_absolute() {
        raw
    } else {
        repo.join(&raw)
    };
    // For new/untracked files canonicalize may fail — fall back to normalized join
    let canon = abs.canonicalize().unwrap_or_else(|_| {
        // Normalize without requiring existence
        let mut out = repo.to_path_buf();
        for c in PathBuf::from(path).components() {
            match c {
                Component::Normal(s) => out.push(s),
                Component::CurDir => {}
                _ => {}
            }
        }
        out
    });
    let repo_canon = repo
        .canonicalize()
        .unwrap_or_else(|_| repo.to_path_buf());
    if !canon.starts_with(&repo_canon) && !abs.starts_with(repo) {
        return Err("路径超出仓库根目录".into());
    }
    let rel = canon
        .strip_prefix(&repo_canon)
        .or_else(|_| abs.strip_prefix(repo))
        .map_err(|_| "路径超出仓库根目录".to_string())?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    if rel_str.is_empty() || rel_str == "." {
        return Err("无效相对路径".into());
    }
    Ok((canon, rel_str))
}

fn merge_in_progress(repo: &Path) -> bool {
    repo.join(".git/MERGE_HEAD").exists()
        || {
            // worktree: MERGE_HEAD may live under git-dir
            git_run(repo, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
                .map(|(_, _, c)| c == 0)
                .unwrap_or(false)
        }
}

fn rebase_in_progress(repo: &Path) -> bool {
    let git_dir = git_ok(repo, &["rev-parse", "--git-path", "rebase-merge"])
        .ok()
        .map(PathBuf::from);
    let git_dir2 = git_ok(repo, &["rev-parse", "--git-path", "rebase-apply"])
        .ok()
        .map(PathBuf::from);
    git_dir.map(|p| repo.join(&p).exists() || p.exists()).unwrap_or(false)
        || git_dir2
            .map(|p| repo.join(&p).exists() || p.exists())
            .unwrap_or(false)
}

fn count_conflicts(repo: &Path) -> u32 {
    conflict_paths_inner(repo).len() as u32
}

fn conflict_paths_inner(repo: &Path) -> Vec<String> {
    let Ok((stdout, _, code)) = git_run(repo, &["diff", "--name-only", "--diff-filter=U"]) else {
        return vec![];
    };
    if code != 0 {
        return vec![];
    }
    stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn is_dirty(repo: &Path) -> bool {
    git_run(repo, &["status", "--porcelain"])
        .map(|(o, _, _)| !o.trim().is_empty())
        .unwrap_or(false)
}

fn merging_ref(repo: &Path) -> Option<String> {
    if !merge_in_progress(repo) {
        return None;
    }
    // MERGE_HEAD is the commit being merged in
    let hash = git_ok(repo, &["rev-parse", "--short", "MERGE_HEAD"]).ok()?;
    let name = git_ok(
        repo,
        &[
            "name-rev",
            "--name-only",
            "--no-undefined",
            "MERGE_HEAD",
        ],
    )
    .ok();
    Some(name.unwrap_or(hash))
}

#[tauri::command]
pub fn git_repo_summary(root_path: String) -> Result<GitRepoSummary, String> {
    let root = PathBuf::from(&root_path);
    if !root.exists() {
        return Ok(GitRepoSummary {
            is_repo: false,
            toplevel: None,
            branch: None,
            upstream: None,
            ahead: 0,
            behind: 0,
            dirty: false,
            merge_in_progress: false,
            rebase_in_progress: false,
            conflict_count: 0,
            merging_into: None,
            error: Some(format!("路径不存在: {root_path}")),
        });
    }

    let (inside, err, code) = match git_run(&root, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(v) => v,
        Err(e) => {
            return Ok(GitRepoSummary {
                is_repo: false,
                toplevel: None,
                branch: None,
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: false,
                merge_in_progress: false,
                rebase_in_progress: false,
                conflict_count: 0,
                merging_into: None,
                error: Some(e),
            });
        }
    };
    if code != 0 || inside.trim() != "true" {
        return Ok(GitRepoSummary {
            is_repo: false,
            toplevel: None,
            branch: None,
            upstream: None,
            ahead: 0,
            behind: 0,
            dirty: false,
            merge_in_progress: false,
            rebase_in_progress: false,
            conflict_count: 0,
            merging_into: None,
            error: Some(if !err.is_empty() {
                err
            } else {
                "不是 git 仓库".into()
            }),
        });
    }

    let repo = resolve_repo(&root_path)?;
    let branch = git_ok(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    let upstream = git_ok(&repo, &["rev-parse", "--abbrev-ref", "@{upstream}"]).ok();
    let (ahead, behind) = if upstream.is_some() {
        match git_ok(&repo, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
            Ok(s) => {
                let mut parts = s.split_whitespace();
                let a = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                let b = parts.next().and_then(|x| x.parse().ok()).unwrap_or(0);
                (a, b)
            }
            Err(_) => (0, 0),
        }
    } else {
        (0, 0)
    };
    let mip = merge_in_progress(&repo);
    let rip = rebase_in_progress(&repo);
    Ok(GitRepoSummary {
        is_repo: true,
        toplevel: Some(repo.to_string_lossy().to_string()),
        branch,
        upstream,
        ahead,
        behind,
        dirty: is_dirty(&repo),
        merge_in_progress: mip,
        rebase_in_progress: rip,
        conflict_count: count_conflicts(&repo),
        merging_into: merging_ref(&repo),
        error: None,
    })
}

/// Parse `git status --porcelain=v1` (simpler than v2 for our needs).
#[tauri::command]
pub fn git_status(root_path: String) -> Result<Vec<GitStatusEntry>, String> {
    let repo = resolve_repo(&root_path)?;
    let (stdout, _, code) = git_run(&repo, &["status", "--porcelain=v1", "-u"])?;
    if code != 0 {
        return Err("git status 失败".into());
    }
    let mut out = Vec::new();
    for line in stdout.lines() {
        if line.len() < 3 {
            continue;
        }
        let xy: String = line.chars().take(2).collect();
        let rest = line[3..].to_string();
        let (path, orig) = if rest.contains(" -> ") {
            let mut parts = rest.splitn(2, " -> ");
            let a = parts.next().unwrap_or("").to_string();
            let b = parts.next().unwrap_or("").to_string();
            (b, Some(a))
        } else {
            (rest, None)
        };
        let x = xy.chars().next().unwrap_or(' ');
        let y = xy.chars().nth(1).unwrap_or(' ');
        let untracked = xy == "??";
        let unmerged = matches!(
            (x, y),
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D')
        );
        let staged = !untracked && !unmerged && x != ' ';
        let unstaged = !untracked && (y != ' ' || unmerged);
        out.push(GitStatusEntry {
            path,
            orig_path: orig,
            status: xy,
            staged,
            unstaged: unstaged || untracked,
            untracked,
            unmerged,
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn git_diff(root_path: String, path: String, staged: bool) -> Result<String, String> {
    let repo = resolve_repo(&root_path)?;
    let (_, rel) = resolve_rel_path(&repo, &path)?;
    let args: Vec<&str> = if staged {
        vec!["diff", "--cached", "--", &rel]
    } else {
        // untracked: show as /dev/null diff via show or just empty
        vec!["diff", "--", &rel]
    };
    let (stdout, stderr, code) = git_run(&repo, &args)?;
    if code != 0 && !stderr.is_empty() && stdout.is_empty() {
        return Err(stderr);
    }
    if stdout.is_empty() && !staged {
        // maybe untracked
        let abs = repo.join(&rel);
        if abs.is_file() {
            let content = fs::read_to_string(&abs).unwrap_or_default();
            if content.is_empty() {
                return Ok(String::new());
            }
            let mut buf = format!("diff --git a/{rel} b/{rel}\n--- /dev/null\n+++ b/{rel}\n");
            for line in content.lines() {
                buf.push('+');
                buf.push_str(line);
                buf.push('\n');
            }
            return Ok(buf);
        }
    }
    Ok(stdout)
}

#[tauri::command]
pub fn git_stage(root_path: String, paths: Vec<String>) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    if paths.is_empty() {
        return Err("未指定文件".into());
    }
    let mut args = vec!["add".to_string(), "--".to_string()];
    for p in &paths {
        let (_, rel) = resolve_rel_path(&repo, p)?;
        args.push(rel);
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    git_ok(&repo, &arg_refs)?;
    Ok(())
}

#[tauri::command]
pub fn git_unstage(root_path: String, paths: Vec<String>) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    if paths.is_empty() {
        return Err("未指定文件".into());
    }
    let mut args = vec!["restore".to_string(), "--staged".to_string(), "--".to_string()];
    for p in &paths {
        let (_, rel) = resolve_rel_path(&repo, p)?;
        args.push(rel);
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    // fallback for older git
    if git_ok(&repo, &arg_refs).is_err() {
        let mut args2 = vec!["reset".to_string(), "HEAD".to_string(), "--".to_string()];
        for p in &paths {
            let (_, rel) = resolve_rel_path(&repo, p)?;
            args2.push(rel);
        }
        let refs2: Vec<&str> = args2.iter().map(|s| s.as_str()).collect();
        git_ok(&repo, &refs2)?;
    }
    Ok(())
}

#[tauri::command]
pub fn git_discard(root_path: String, paths: Vec<String>) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    if paths.is_empty() {
        return Err("未指定文件".into());
    }
    let conflicts = conflict_paths_inner(&repo);
    for p in &paths {
        let (_, rel) = resolve_rel_path(&repo, p)?;
        if conflicts.iter().any(|c| c == &rel) {
            return Err(format!("冲突未解决，禁止丢弃: {rel}"));
        }
    }
    // Separate untracked vs tracked
    let status = git_status(root_path.clone())?;
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    for p in &paths {
        let (_, rel) = resolve_rel_path(&repo, p)?;
        let entry = status.iter().find(|e| e.path == rel);
        if entry.map(|e| e.untracked).unwrap_or(false) {
            untracked.push(rel);
        } else {
            tracked.push(rel);
        }
    }
    if !tracked.is_empty() {
        let mut args = vec!["restore".to_string(), "--worktree".to_string(), "--".to_string()];
        args.extend(tracked.clone());
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        if git_ok(&repo, &refs).is_err() {
            let mut args2 = vec!["checkout".to_string(), "--".to_string()];
            args2.extend(tracked);
            let refs2: Vec<&str> = args2.iter().map(|s| s.as_str()).collect();
            git_ok(&repo, &refs2)?;
        }
    }
    for rel in untracked {
        let abs = repo.join(&rel);
        if abs.is_file() {
            fs::remove_file(&abs).map_err(|e| format!("删除失败 {rel}: {e}"))?;
        } else if abs.is_dir() {
            fs::remove_dir_all(&abs).map_err(|e| format!("删除失败 {rel}: {e}"))?;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn git_commit(root_path: String, message: String, amend: bool) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    let msg = message.trim();
    if msg.is_empty() && !amend {
        return Err("提交说明不能为空".into());
    }
    if amend {
        // Only allow amend when HEAD is not pushed (ahead > 0 or no upstream)
        let summary = git_repo_summary(root_path.clone())?;
        if summary.upstream.is_some() && summary.ahead == 0 && !summary.merge_in_progress {
            return Err("HEAD 已与上游同步，禁止 amend（避免改写已推送提交）".into());
        }
        if msg.is_empty() {
            git_ok(&repo, &["commit", "--amend", "--no-edit"])?;
        } else {
            git_ok(&repo, &["commit", "--amend", "-m", msg])?;
        }
    } else {
        git_ok(&repo, &["commit", "-m", msg])?;
    }
    Ok(())
}

#[tauri::command]
pub fn git_branches(root_path: String) -> Result<Vec<GitBranchInfo>, String> {
    let repo = resolve_repo(&root_path)?;
    // format: refname | upstream | ahead | behind | current(*)
    let fmt = "%(refname:short)\t%(upstream:short)\t%(upstream:track)\t%(HEAD)";
    let (stdout, _, code) = git_run(
        &repo,
        &["for-each-ref", "--format", fmt, "refs/heads/", "refs/remotes/"],
    )?;
    if code != 0 {
        return Err("列出分支失败".into());
    }
    let mut out = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.is_empty() || parts[0].is_empty() {
            continue;
        }
        let name = parts[0].to_string();
        if name.ends_with("/HEAD") {
            continue;
        }
        let is_remote = git_run(
            &repo,
            &["show-ref", "--verify", &format!("refs/remotes/{name}")],
        )
        .map(|(_, _, c)| c == 0)
        .unwrap_or(false);

        let upstream = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let track = parts.get(2).copied().unwrap_or("");
        let (ahead, behind) = parse_track(track);
        let is_current = parts.get(3).copied() == Some("*");
        out.push(GitBranchInfo {
            name,
            is_current,
            is_remote,
            upstream,
            ahead,
            behind,
        });
    }
    // Prefer locals first
    out.sort_by(|a, b| match (a.is_remote, b.is_remote) {
        (false, true) => std::cmp::Ordering::Less,
        (true, false) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    Ok(out)
}

fn parse_track(track: &str) -> (u32, u32) {
    // e.g. [ahead 1, behind 2] or [ahead 1] or [behind 2]
    let mut ahead = 0u32;
    let mut behind = 0u32;
    if let Some(i) = track.find("ahead ") {
        ahead = track[i + 6..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
    }
    if let Some(i) = track.find("behind ") {
        behind = track[i + 7..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
    }
    (ahead, behind)
}

#[tauri::command]
pub fn git_checkout(root_path: String, branch: String) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    let b = branch.trim();
    if b.is_empty() {
        return Err("分支名为空".into());
    }
    git_ok(&repo, &["checkout", b])?;
    Ok(())
}

#[tauri::command]
pub fn git_create_branch(root_path: String, name: String, checkout: bool) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    let n = name.trim();
    if n.is_empty() {
        return Err("分支名为空".into());
    }
    if checkout {
        git_ok(&repo, &["checkout", "-b", n])?;
    } else {
        git_ok(&repo, &["branch", n])?;
    }
    Ok(())
}

#[tauri::command]
pub fn git_log(root_path: String, limit: Option<u32>) -> Result<Vec<GitCommitInfo>, String> {
    let repo = resolve_repo(&root_path)?;
    let lim = limit.unwrap_or(50).clamp(1, 200).to_string();
    let (stdout, _, code) = git_run(
        &repo,
        &[
            "log",
            &format!("-n{lim}"),
            "--format=%H%x09%h%x09%s%x09%an%x09%ar",
        ],
    )?;
    if code != 0 {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 5 {
            continue;
        }
        out.push(GitCommitInfo {
            hash: parts[0].to_string(),
            short_hash: parts[1].to_string(),
            subject: parts[2].to_string(),
            author: parts[3].to_string(),
            relative_time: parts[4].to_string(),
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn git_show(root_path: String, rev: String) -> Result<String, String> {
    let repo = resolve_repo(&root_path)?;
    let r = rev.trim();
    if r.is_empty() {
        return Err("rev 为空".into());
    }
    git_ok(&repo, &["show", "--stat", "--patch", "--format=fuller", r])
}

fn op_result(stdout: String, stderr: String, code: i32, repo: &Path) -> GitOpResult {
    let conflict_paths = conflict_paths_inner(repo);
    let has_conflict = !conflict_paths.is_empty();
    GitOpResult {
        ok: code == 0 && !has_conflict,
        stdout,
        stderr,
        conflicts: has_conflict,
        conflict_paths,
    }
}

#[tauri::command]
pub fn git_fetch(root_path: String) -> Result<GitOpResult, String> {
    let repo = resolve_repo(&root_path)?;
    let (stdout, stderr, code) = git_run(&repo, &["fetch", "--prune"])?;
    Ok(op_result(stdout, stderr, code, &repo))
}

#[tauri::command]
pub fn git_pull(root_path: String) -> Result<GitOpResult, String> {
    let repo = resolve_repo(&root_path)?;
    let (stdout, stderr, code) = git_run(&repo, &["pull", "--no-edit"])?;
    Ok(op_result(stdout, stderr, code, &repo))
}

#[tauri::command]
pub fn git_push(root_path: String) -> Result<GitOpResult, String> {
    let repo = resolve_repo(&root_path)?;
    let (stdout, stderr, code) = git_run(&repo, &["push"])?;
    Ok(GitOpResult {
        ok: code == 0,
        stdout,
        stderr,
        conflicts: false,
        conflict_paths: vec![],
    })
}

#[tauri::command]
pub fn git_merge(root_path: String, branch: String) -> Result<GitOpResult, String> {
    let repo = resolve_repo(&root_path)?;
    let b = branch.trim();
    if b.is_empty() {
        return Err("分支名为空".into());
    }
    let (stdout, stderr, code) = git_run(&repo, &["merge", "--no-edit", b])?;
    Ok(op_result(stdout, stderr, code, &repo))
}

#[tauri::command]
pub fn git_merge_abort(root_path: String) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    git_ok(&repo, &["merge", "--abort"])?;
    Ok(())
}

#[tauri::command]
pub fn git_merge_continue(root_path: String, message: Option<String>) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    let left = conflict_paths_inner(&repo);
    if !left.is_empty() {
        return Err(format!("仍有 {} 个未解决冲突", left.len()));
    }
    if !merge_in_progress(&repo) {
        return Err("当前不在合并状态".into());
    }
    let msg = message
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    if let Some(m) = msg {
        git_ok(&repo, &["commit", "--no-edit", "-m", &m])?;
    } else {
        // Use MERGE_MSG
        git_ok(&repo, &["commit", "--no-edit"])?;
    }
    Ok(())
}

#[tauri::command]
pub fn git_conflict_paths(root_path: String) -> Result<Vec<String>, String> {
    let repo = resolve_repo(&root_path)?;
    Ok(conflict_paths_inner(&repo))
}

fn show_stage(repo: &Path, stage: u8, rel: &str) -> Option<String> {
    let spec = format!(":{stage}:{rel}");
    match git_run(repo, &["show", &spec]) {
        Ok((stdout, _, code)) if code == 0 => Some(stdout),
        _ => None,
    }
}

#[tauri::command]
pub fn git_conflict_sides(root_path: String, path: String) -> Result<GitConflictSides, String> {
    let repo = resolve_repo(&root_path)?;
    let (abs, rel) = resolve_rel_path(&repo, &path)?;
    let base = show_stage(&repo, 1, &rel);
    let ours = show_stage(&repo, 2, &rel);
    let theirs = show_stage(&repo, 3, &rel);
    let worktree = if abs.is_file() {
        fs::read_to_string(&abs).ok()
    } else {
        None
    };
    Ok(GitConflictSides {
        path: rel,
        has_stage1: base.is_some(),
        has_stage2: ours.is_some(),
        has_stage3: theirs.is_some(),
        base,
        ours,
        theirs,
        worktree,
    })
}

#[tauri::command]
pub fn git_write_file(root_path: String, path: String, content: String) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    let (abs, _) = resolve_rel_path(&repo, &path)?;
    if let Some(parent) = abs.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    fs::write(&abs, content.as_bytes()).map_err(|e| format!("写入失败: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn git_stash_list(root_path: String) -> Result<Vec<GitStashEntry>, String> {
    let repo = resolve_repo(&root_path)?;
    let (stdout, _, code) = git_run(&repo, &["stash", "list", "--format=%gd\t%s"])?;
    if code != 0 {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for (i, line) in stdout.lines().enumerate() {
        let msg = if let Some((_, rest)) = line.split_once('\t') {
            rest.to_string()
        } else {
            line.to_string()
        };
        out.push(GitStashEntry {
            index: i as u32,
            message: msg,
        });
    }
    Ok(out)
}

#[tauri::command]
pub fn git_stash_push(root_path: String, message: Option<String>) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    if let Some(m) = message.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        git_ok(&repo, &["stash", "push", "-m", m])?;
    } else {
        git_ok(&repo, &["stash", "push"])?;
    }
    Ok(())
}

#[tauri::command]
pub fn git_stash_apply(root_path: String, index: u32) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    let r = format!("stash@{{{index}}}");
    git_ok(&repo, &["stash", "apply", &r])?;
    Ok(())
}

#[tauri::command]
pub fn git_stash_drop(root_path: String, index: u32) -> Result<(), String> {
    let repo = resolve_repo(&root_path)?;
    let r = format!("stash@{{{index}}}");
    git_ok(&repo, &["stash", "drop", &r])?;
    Ok(())
}

#[tauri::command]
pub fn git_default_merge_message(root_path: String) -> Result<String, String> {
    let repo = resolve_repo(&root_path)?;
    let path = git_ok(&repo, &["rev-parse", "--git-path", "MERGE_MSG"])?;
    let p = PathBuf::from(&path);
    let abs = if p.is_absolute() {
        p
    } else {
        repo.join(p)
    };
    fs::read_to_string(&abs).or_else(|_| Ok("Merge branch".into()))
}
