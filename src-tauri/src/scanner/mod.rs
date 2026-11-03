mod spring_boot;
mod vue;

use crate::model::{Launchable, Project, ProjectKind, ScanProgress};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    ".idea",
    "dist",
    "build",
    ".svn",
    "vendor",
    "project-launcher",
    "sparkrun",
    ".turbo",
    "coverage",
];

pub fn should_skip_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|s| *s == name)
}

pub type ProgressFn<'a> = dyn FnMut(ScanProgress) + Send + 'a;

/// Scan a project root with enabled kind detectors (extensible registry).
#[allow(dead_code)]
pub fn scan_project(project: &Project) -> Result<Vec<Launchable>, String> {
    scan_project_with_progress(project, &mut |_| {})
}

pub fn scan_project_with_progress(
    project: &Project,
    progress: &mut ProgressFn<'_>,
) -> Result<Vec<Launchable>, String> {
    let root = PathBuf::from(&project.root_path);
    if !root.is_dir() {
        return Err(format!(
            "project root is not a directory: {}",
            project.root_path
        ));
    }
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
    let max_depth = project.effective_scan_depth();
    let kinds = project.enabled_kinds();
    let project_id = project.id.clone();

    let mut units = Vec::new();

    if kinds.contains(&ProjectKind::SpringBoot) {
        progress(ScanProgress {
            project_id: project_id.clone(),
            phase: "springBoot".into(),
            current: 0,
            total: None,
            found: 0,
            message: "扫描 Spring Boot…".into(),
        });
        let boot = spring_boot::scan(&root, &root_canon, max_depth, &mut |cur, found| {
            progress(ScanProgress {
                project_id: project_id.clone(),
                phase: "springBoot".into(),
                current: cur,
                total: None,
                found,
                message: format!("扫描 Spring Boot… 已检查 {cur} · 发现 {found}"),
            });
        })?;
        units.extend(boot);
    }
    if kinds.contains(&ProjectKind::Vue) {
        let found_so_far = units.len() as u32;
        progress(ScanProgress {
            project_id: project_id.clone(),
            phase: "vue".into(),
            current: 0,
            total: None,
            found: found_so_far,
            message: "扫描 Vue…".into(),
        });
        let vue_units = vue::scan(&root, &root_canon, max_depth, &mut |cur, found| {
            progress(ScanProgress {
                project_id: project_id.clone(),
                phase: "vue".into(),
                current: cur,
                total: None,
                found: found_so_far + found,
                message: format!("扫描 Vue… 已检查 {cur} · 发现 {found}"),
            });
        })?;
        units.extend(vue_units);
    }

    units.sort_by(|a, b| {
        a.kind
            .as_str()
            .cmp(b.kind.as_str())
            .then(a.relative_path.cmp(&b.relative_path))
    });

    let n = units.len() as u32;
    progress(ScanProgress {
        project_id,
        phase: "done".into(),
        current: n,
        total: Some(n),
        found: n,
        message: format!("扫描完成 · {n} 个单元"),
    });
    Ok(units)
}

pub fn walk_files<'a>(
    root: &'a Path,
    max_depth: usize,
    file_name: &'a str,
) -> impl Iterator<Item = PathBuf> + 'a {
    WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                !should_skip_dir(&e.file_name().to_string_lossy())
            } else {
                true
            }
        })
        .filter_map(|e| e.ok())
        .filter(move |e| e.file_type().is_file() && e.file_name() == file_name)
        .map(|e| e.path().to_path_buf())
}

pub fn relative_to(root_canon: &Path, abs: &Path) -> String {
    abs.strip_prefix(root_canon)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Project;

    #[test]
    fn scan_repo_finds_boot_and_vue() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .canonicalize()
            .unwrap();
        let mut project = Project::new("ypyh".into(), repo.to_string_lossy().to_string());
        project.scan_depth = 3;
        let units = scan_project(&project).unwrap();
        assert!(
            units.iter().any(|u| u.kind == ProjectKind::SpringBoot
                && (u.name.contains("growup100-api") || u.relative_path.contains("growup100-api"))),
            "missing spring boot api"
        );
        assert!(
            units.iter().any(|u| u.kind == ProjectKind::Vue),
            "expected at least one Vue unit, got kinds: {:?}",
            units.iter().map(|u| u.kind.as_str()).collect::<Vec<_>>()
        );
    }
}
