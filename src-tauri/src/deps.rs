use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

/// Strip Windows extended-length prefix `\\?\` for stable path matching / display.
pub fn normalize_path(path: &str) -> String {
    let p = path.trim();
    if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.replace('/', "\\")
    } else {
        p.replace('/', "\\")
    }
}

pub(crate) fn strip_block(src: &str, tag: &str) -> String {
    let pattern = format!(r"(?s)<{tag}>.*?</{tag}>");
    match Regex::new(&pattern) {
        Ok(re) => re.replace_all(src, "").to_string(),
        Err(_) => src.to_string(),
    }
}

fn extract_dependency_artifact_ids(pom: &str) -> Vec<String> {
    // Keep only <dependencies>…</dependencies>, drop dependencyManagement / parent / build
    let deps_block = Regex::new(r"(?s)<dependencies>(.*?)</dependencies>")
        .ok()
        .and_then(|re| {
            re.captures(pom)
                .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        })
        .unwrap_or_default();

    let art_re = match Regex::new(r"(?s)<dependency>.*?<artifactId>\s*([^<]+?)\s*</artifactId>.*?</dependency>")
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    art_re
        .captures_iter(&deps_block)
        .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect()
}

fn module_dir_for_artifact(workspace: &Path, artifact_id: &str) -> Option<PathBuf> {
    let candidate = workspace.join(artifact_id);
    if candidate.join("pom.xml").is_file() {
        return Some(candidate);
    }
    None
}

/// Resolve workspace-local Maven modules that `module_dir`'s pom depends on,
/// in install order (dependencies before dependents).
pub fn resolve_local_install_order(
    workspace: &Path,
    module_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let module_dir = module_dir
        .canonicalize()
        .unwrap_or_else(|_| module_dir.to_path_buf());

    // artifactId -> module path
    let mut graph: Vec<(String, PathBuf, Vec<String>)> = Vec::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    let mut seen: HashSet<String> = HashSet::new();

    queue.push_back(module_dir.clone());

    while let Some(dir) = queue.pop_front() {
        let pom_path = dir.join("pom.xml");
        let pom = fs::read_to_string(&pom_path)
            .map_err(|e| format!("read {}: {e}", pom_path.display()))?;
        let cleaned = strip_block(&pom, "parent");
        let cleaned = strip_block(&cleaned, "dependencyManagement");
        let cleaned = strip_block(&cleaned, "build");
        let cleaned = strip_block(&cleaned, "profiles");

        let art_re = Regex::new(r"(?s)<artifactId>\s*([^<]+?)\s*</artifactId>")
            .map_err(|e| e.to_string())?;
        // first artifactId after stripping parent is project artifactId
        let artifact_id = art_re
            .captures(&cleaned)
            .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .unwrap_or_else(|| {
                dir.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            });

        if !seen.insert(artifact_id.clone()) {
            continue;
        }

        let dep_ids = extract_dependency_artifact_ids(&pom);
        let mut local_deps = Vec::new();
        for dep in &dep_ids {
            if let Some(dep_dir) = module_dir_for_artifact(&workspace, dep) {
                local_deps.push(dep.clone());
                queue.push_back(dep_dir);
            }
        }
        graph.push((artifact_id, dir, local_deps));
    }

    // Kahn topological sort over local modules; exclude the target module itself from install list
    let target_key = graph
        .iter()
        .find(|(_, p, _)| paths_equal(p, &module_dir))
        .map(|(id, _, _)| id.clone());

    let ids: HashSet<String> = graph.iter().map(|(id, _, _)| id.clone()).collect();
    let mut indegree: std::collections::HashMap<String, usize> = ids.iter().map(|id| (id.clone(), 0)).collect();
    let mut adj: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

    for (id, _, deps) in &graph {
        for d in deps {
            if ids.contains(d) {
                // edge: d -> id (install d first)
                adj.entry(d.clone()).or_default().push(id.clone());
                *indegree.entry(id.clone()).or_insert(0) += 1;
            }
        }
    }

    let mut ready: VecDeque<String> = indegree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let path_by_id: std::collections::HashMap<String, PathBuf> = graph
        .iter()
        .map(|(id, p, _)| (id.clone(), p.clone()))
        .collect();

    let mut order = Vec::new();
    while let Some(id) = ready.pop_front() {
        if let Some(ref t) = target_key {
            if &id != t {
                if let Some(p) = path_by_id.get(&id) {
                    order.push(p.clone());
                }
            }
        }
        if let Some(nexts) = adj.get(&id) {
            for n in nexts {
                if let Some(deg) = indegree.get_mut(n) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        ready.push_back(n.clone());
                    }
                }
            }
        }
    }

    Ok(order)
}

/// Local dependency modules as `(artifactId, moduleDir)` in install order.
pub fn resolve_local_deps(
    workspace: &Path,
    module_dir: &Path,
) -> Result<Vec<(String, PathBuf)>, String> {
    let order = resolve_local_install_order(workspace, module_dir)?;
    let mut out = Vec::new();
    for dir in order {
        let pom_path = dir.join("pom.xml");
        let pom = fs::read_to_string(&pom_path).unwrap_or_default();
        let cleaned = strip_block(&pom, "parent");
        let cleaned = strip_block(&cleaned, "dependencies");
        let cleaned = strip_block(&cleaned, "dependencyManagement");
        let cleaned = strip_block(&cleaned, "build");
        let cleaned = strip_block(&cleaned, "profiles");
        let art_re = Regex::new(r"(?s)<artifactId>\s*([^<]+?)\s*</artifactId>")
            .map_err(|e| e.to_string())?;
        let id = art_re
            .captures(&cleaned)
            .and_then(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .unwrap_or_else(|| {
                dir.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            });
        out.push((id, dir));
    }
    Ok(out)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    normalize_path(&a.to_string_lossy())
        .eq_ignore_ascii_case(&normalize_path(&b.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_depends_on_business() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .canonicalize()
            .unwrap();
        let api = workspace.join("growup100-api");
        let order = resolve_local_install_order(&workspace, &api).unwrap();
        assert!(
            order.iter().any(|p| p.ends_with("growup100-business")),
            "expected growup100-business in {:?}",
            order
        );
    }
}
