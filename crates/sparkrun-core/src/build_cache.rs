use std::fs;
use std::path::Path;
use std::time::SystemTime;
use walkdir::WalkDir;

pub const STAMP_FILE: &str = ".launcher-compile-stamp";

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn classes_dir(module_dir: &Path) -> std::path::PathBuf {
    module_dir.join("target").join("classes")
}

fn stamp_path(module_dir: &Path) -> std::path::PathBuf {
    module_dir.join(STAMP_FILE)
}

/// Baseline time used to detect newer sources: stamp file if present, else target/classes mtime.
fn baseline(module_dir: &Path) -> Option<SystemTime> {
    let stamp = stamp_path(module_dir);
    if stamp.is_file() {
        return mtime(&stamp);
    }
    let classes = classes_dir(module_dir);
    if classes.is_dir() {
        return mtime(&classes);
    }
    None
}

fn is_newer_than(path: &Path, baseline: SystemTime) -> bool {
    mtime(path).map(|t| t > baseline).unwrap_or(false)
}

/// True when module needs `mvn compile` (missing output or sources/pom newer than last build).
pub fn is_module_stale(module_dir: &Path) -> bool {
    let classes = classes_dir(module_dir);
    if !classes.is_dir() {
        return true;
    }
    let Some(base) = baseline(module_dir) else {
        return true;
    };

    let pom = module_dir.join("pom.xml");
    if pom.is_file() && is_newer_than(&pom, base) {
        return true;
    }

    for sub in ["java", "resources"] {
        let root = module_dir.join("src").join("main").join(sub);
        if !root.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&root)
            .max_depth(32)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            if is_newer_than(entry.path(), base) {
                return true;
            }
        }
    }
    false
}

pub fn write_compile_stamp(module_dir: &Path) -> Result<(), String> {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".into());
    fs::write(stamp_path(module_dir), now).map_err(|e| format!("write stamp: {e}"))
}

pub fn ensure_classes_exist(module_dir: &Path) -> Result<(), String> {
    let classes = classes_dir(module_dir);
    if classes.is_dir() {
        Ok(())
    } else {
        Err(format!(
            "missing compiled output: {} — please Build first",
            classes.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn missing_classes_is_stale() {
        let dir = std::env::temp_dir().join(format!(
            "launcher_stale_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(is_module_stale(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_classes_not_stale() {
        let dir = std::env::temp_dir().join(format!(
            "launcher_fresh_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let classes = dir.join("target").join("classes");
        fs::create_dir_all(&classes).unwrap();
        write_compile_stamp(&dir).unwrap();
        // source older than stamp: create then touch stamp again
        let src = dir.join("src").join("main").join("java");
        fs::create_dir_all(&src).unwrap();
        let mut f = fs::File::create(src.join("A.java")).unwrap();
        writeln!(f, "class A {{}}").unwrap();
        // Make stamp newer than source
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_compile_stamp(&dir).unwrap();
        assert!(!is_module_stale(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
