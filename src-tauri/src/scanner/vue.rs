use crate::deps::normalize_path;
use crate::model::{Launchable, ProjectKind};
use crate::scanner::{relative_to, walk_files};
use serde_json::Value;
use std::fs;
use std::path::Path;

fn detect_package_manager(dir: &Path) -> String {
    if dir.join("pnpm-lock.yaml").exists() {
        return "pnpm".into();
    }
    if dir.join("yarn.lock").exists() {
        return "yarn".into();
    }
    if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        return "bun".into();
    }
    "npm".into()
}

fn pick_start_script(scripts: &Value) -> Option<String> {
    let obj = scripts.as_object()?;
    for key in ["dev", "serve", "start"] {
        if obj.contains_key(key) {
            return Some(key.to_string());
        }
    }
    None
}

fn has_vue_dep(pkg: &Value) -> bool {
    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(deps) = pkg.get(section).and_then(|v| v.as_object()) {
            if deps.contains_key("vue")
                || deps.contains_key("@vue/cli-service")
                || deps.contains_key("@vitejs/plugin-vue")
                || deps.contains_key("wepy")
                || deps.keys().any(|k| k.starts_with("@vue/"))
            {
                return true;
            }
        }
    }
    false
}

pub fn scan(
    root: &Path,
    root_canon: &Path,
    max_depth: usize,
    on_tick: &mut dyn FnMut(u32, u32),
) -> Result<Vec<Launchable>, String> {
    let mut units = Vec::new();
    let mut checked: u32 = 0;

    for pkg_path in walk_files(root, max_depth, "package.json") {
        checked += 1;
        if checked == 1 || checked % 8 == 0 {
            on_tick(checked, units.len() as u32);
        }
        let module_dir = match pkg_path.parent() {
            Some(p) => p,
            None => continue,
        };
        // Skip nested package.json inside already-skipped trees (walk handles skip dirs)
        let raw = match fs::read_to_string(&pkg_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let pkg: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !has_vue_dep(&pkg) {
            continue;
        }
        let scripts = pkg.get("scripts").cloned().unwrap_or(Value::Null);
        let start_script = match pick_start_script(&scripts) {
            Some(s) => s,
            None => continue, // no runnable script
        };

        let abs = module_dir
            .canonicalize()
            .unwrap_or_else(|_| module_dir.to_path_buf());
        let path = normalize_path(&abs.to_string_lossy());
        let relative = relative_to(root_canon, &abs);
        let name = pkg
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                module_dir
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "vue-app".to_string())
            });
        let pm = detect_package_manager(module_dir);

        units.push(Launchable {
            id: Launchable::make_id(ProjectKind::Vue, &path),
            kind: ProjectKind::Vue,
            name,
            path,
            relative_path: if relative.is_empty() {
                ".".to_string()
            } else {
                relative
            },
            detail: Some(format!("{pm} run {start_script}")),
            package_manager: Some(pm),
            start_script: Some(start_script),
            spring_profiles: Vec::new(),
            selected_spring_profile: "application".to_string(),
        });
    }
    on_tick(checked, units.len() as u32);

    Ok(units)
}
