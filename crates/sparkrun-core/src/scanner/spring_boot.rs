use crate::deps::normalize_path;
use crate::model::{Launchable, ProjectKind};
use crate::scanner::{relative_to, walk_files};
use regex::Regex;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

/// Detect Spring Boot config profiles from `src/main/resources/application*.{yml,yaml,properties}`.
/// First entry is always sentinel `"application"` (base config / no spring.profiles.active).
pub fn detect_spring_profiles(module_dir: &Path) -> Vec<String> {
    let resources = module_dir.join("src").join("main").join("resources");
    let mut profiles: BTreeSet<String> = BTreeSet::new();

    if resources.is_dir() {
        if let Ok(rd) = fs::read_dir(&resources) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let lower = name.to_lowercase();
                let stem = if lower.ends_with(".yml") {
                    &name[..name.len() - 4]
                } else if lower.ends_with(".yaml") {
                    &name[..name.len() - 5]
                } else if lower.ends_with(".properties") {
                    &name[..name.len() - 11]
                } else {
                    continue;
                };
                let stem_lower = stem.to_lowercase();
                if stem_lower == "application" {
                    continue;
                }
                if let Some(idx) = stem_lower.find("application-") {
                    if idx == 0 {
                        let raw = &stem["application-".len()..];
                        if !raw.is_empty() && !raw.contains('.') {
                            profiles.insert(raw.to_string());
                        }
                    }
                }
            }
        }
    }

    let mut out = vec!["application".to_string()];
    out.extend(profiles);
    out
}

fn strip_block(src: &str, tag: &str) -> String {
    let pattern = format!(r"(?s)<{tag}>.*?</{tag}>");
    match Regex::new(&pattern) {
        Ok(re) => re.replace_all(src, "").to_string(),
        Err(_) => src.to_string(),
    }
}

fn extract_artifact_id(pom: &str) -> Option<String> {
    let cleaned = strip_block(pom, "parent");
    let cleaned = strip_block(&cleaned, "dependencies");
    let cleaned = strip_block(&cleaned, "dependencyManagement");
    let cleaned = strip_block(&cleaned, "build");
    let cleaned = strip_block(&cleaned, "profiles");
    let re = Regex::new(r"(?s)<artifactId>\s*([^<]+?)\s*</artifactId>").ok()?;
    re.captures(&cleaned)
        .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
}

fn is_boot_parent_or_plugin(pom: &str) -> bool {
    pom.contains("spring-boot-starter-parent") || pom.contains("spring-boot-maven-plugin")
}

fn find_spring_boot_main(module_dir: &Path) -> Option<String> {
    let java_root = module_dir.join("src").join("main").join("java");
    if !java_root.exists() {
        return None;
    }
    let re = Regex::new(r"@SpringBootApplication").ok()?;
    let package_re = Regex::new(r"package\s+([\w.]+)\s*;").ok()?;
    let class_re = Regex::new(r"(?:public\s+)?(?:final\s+)?class\s+(\w+)").ok()?;

    for entry in WalkDir::new(&java_root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s == "java")
                .unwrap_or(false)
        })
    {
        let content = match fs::read_to_string(entry.path()) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !re.is_match(&content) {
            continue;
        }
        let pkg = package_re
            .captures(&content)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
            .unwrap_or_default();
        let cls = class_re
            .captures(&content)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))?;
        if pkg.is_empty() {
            return Some(cls);
        }
        return Some(format!("{pkg}.{cls}"));
    }
    None
}

pub fn scan(
    root: &Path,
    root_canon: &Path,
    max_depth: usize,
    on_tick: &mut dyn FnMut(u32, u32),
) -> Result<Vec<Launchable>, String> {
    let mut modules = Vec::new();
    let mut checked: u32 = 0;

    for pom_path in walk_files(root, max_depth, "pom.xml") {
        checked += 1;
        if checked == 1 || checked % 8 == 0 {
            on_tick(checked, modules.len() as u32);
        }
        let module_dir = match pom_path.parent() {
            Some(p) => p,
            None => continue,
        };
        let pom = match fs::read_to_string(&pom_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let main_class = find_spring_boot_main(module_dir);
        if main_class.is_none() && !is_boot_parent_or_plugin(&pom) {
            continue;
        }

        let artifact_id = extract_artifact_id(&pom);
        let abs = module_dir
            .canonicalize()
            .unwrap_or_else(|_| module_dir.to_path_buf());
        let path = normalize_path(&abs.to_string_lossy());
        let relative = relative_to(root_canon, &abs);
        let name = artifact_id.clone().unwrap_or_else(|| {
            module_dir
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        });

        let spring_profiles = detect_spring_profiles(module_dir);
        modules.push(Launchable {
            id: Launchable::make_id(ProjectKind::SpringBoot, &path),
            kind: ProjectKind::SpringBoot,
            name,
            path,
            relative_path: if relative.is_empty() {
                ".".to_string()
            } else {
                relative
            },
            detail: main_class,
            package_manager: None,
            start_script: None,
            spring_profiles,
            selected_spring_profile: "application".to_string(),
        });
    }
    on_tick(checked, modules.len() as u32);

    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_profiles_in_api() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("growup100-api");
        let profiles = detect_spring_profiles(&repo);
        assert!(profiles.contains(&"application".to_string()));
        assert!(profiles.iter().any(|p| p == "dev" || p == "prod"));
    }
}
