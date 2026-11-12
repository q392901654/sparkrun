//! Shorten overlong Java classpaths (Windows CreateProcess limit ~8191 → OS error 206)
//! by writing a classpath JAR whose MANIFEST.MF lists entries (IDEA "JAR manifest" mode).
//! Compatible with Java 8+.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// If `classpath` is too long for the OS command line, write a classpath jar and return its path.
/// Otherwise return `classpath` unchanged.
pub fn shorten_classpath_arg(
    module_dir: &Path,
    classpath: &str,
    _log: &dyn Fn(&str, String),
) -> Result<String, String> {
    // Windows CreateProcess command-line limit is 8191 chars (OS error 206).
    let must_shorten = if cfg!(windows) {
        classpath.len() >= 4_000 || classpath.matches(';').count() >= 40
    } else {
        classpath.len() >= 100_000
    };
    if !must_shorten {
        return Ok(classpath.to_string());
    }

    let sep = if cfg!(windows) { ';' } else { ':' };
    let entries: Vec<PathBuf> = classpath
        .split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    if entries.is_empty() {
        return Err("empty classpath".into());
    }

    let out_dir = module_dir.join("target");
    fs::create_dir_all(&out_dir).map_err(|e| format!("create target dir: {e}"))?;
    let jar_path = out_dir.join(".launcher-classpath.jar");

    write_classpath_jar(&jar_path, &entries)?;
    Ok(jar_path.to_string_lossy().to_string())
}

fn path_to_manifest_url(path: &Path) -> String {
    // Match IDEA / File.toURI(): file:/C:/foo/bar.jar
    let abs = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut s = abs.to_string_lossy().to_string();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        s = stripped.to_string();
    }
    s = s.replace('\\', "/");
    s = s.replace(' ', "%20");

    let is_dir = path.is_dir() || abs.is_dir();
    let mut url = if cfg!(windows) {
        format!("file:/{s}")
    } else if s.starts_with('/') {
        format!("file:{s}")
    } else {
        format!("file:/{s}")
    };
    if is_dir && !url.ends_with('/') {
        url.push('/');
    }
    url
}

/// Manifest attribute values must be wrapped at 72 bytes; continuation lines start with a space.
/// Continuation leading space is *removed* when joining — so we hard-wrap mid-stream like IDEA,
/// keeping spaces between URLs inside the payload (never wrap *between* tokens with a bare newline).
fn build_manifest(class_path_urls: &[String]) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("Manifest-Version: 1.0\r\n");
    out.push_str("Created-By: sparkrun\r\n");
    let value = class_path_urls.join(" ");
    write_wrapped_header(&mut out, "Class-Path", &value);
    out.push_str("\r\n");
    out.into_bytes()
}

fn write_wrapped_header(out: &mut String, name: &str, value: &str) {
    // Continuous "Name: value…" then hard-wrap every 72 bytes (JAR spec).
    let mut remaining = format!("{name}: {value}");
    loop {
        if remaining.len() <= 72 {
            out.push_str(&remaining);
            out.push_str("\r\n");
            break;
        }
        let (head, rest) = remaining.split_at(72);
        out.push_str(head);
        out.push_str("\r\n");
        // Leading space marks continuation; parser strips exactly one space when joining.
        remaining = format!(" {rest}");
    }
}

fn write_classpath_jar(jar_path: &Path, entries: &[PathBuf]) -> Result<(), String> {
    let urls: Vec<String> = entries.iter().map(|p| path_to_manifest_url(p)).collect();
    let manifest = build_manifest(&urls);
    write_single_file_jar(jar_path, "META-INF/MANIFEST.MF", &manifest)
}

fn write_single_file_jar(jar_path: &Path, entry_name: &str, data: &[u8]) -> Result<(), String> {
    let mut buf: Vec<u8> = Vec::new();
    let name_bytes = entry_name.as_bytes();
    let crc = crc32(data);
    let size = data.len() as u32;

    buf.extend_from_slice(&0x04034b50u32.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(&size.to_le_bytes());
    buf.extend_from_slice(&size.to_le_bytes());
    buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(name_bytes);
    buf.extend_from_slice(data);
    let local_header_offset = 0u32;

    let central_offset = buf.len() as u32;
    buf.extend_from_slice(&0x02014b50u32.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes());
    buf.extend_from_slice(&20u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes());
    buf.extend_from_slice(&size.to_le_bytes());
    buf.extend_from_slice(&size.to_le_bytes());
    buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes());
    buf.extend_from_slice(&local_header_offset.to_le_bytes());
    buf.extend_from_slice(name_bytes);

    let central_size = buf.len() as u32 - central_offset;
    buf.extend_from_slice(&0x06054b50u32.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&central_size.to_le_bytes());
    buf.extend_from_slice(&central_offset.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes());

    let mut f = fs::File::create(jar_path).map_err(|e| format!("create classpath jar: {e}"))?;
    f.write_all(&buf)
        .map_err(|e| format!("write classpath jar: {e}"))?;
    Ok(())
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Unfold JAR manifest Class-Path for tests / diagnostics.
#[cfg(test)]
fn unfold_classpath_attr(manifest: &str) -> String {
    let mut joined = String::new();
    let mut in_cp = false;
    for line in manifest.split("\r\n") {
        if let Some(rest) = line.strip_prefix("Class-Path: ") {
            in_cp = true;
            joined.push_str(rest);
        } else if in_cp && line.starts_with(' ') {
            joined.push_str(&line[1..]);
        } else if in_cp {
            break;
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn wrap_keeps_space_between_urls() {
        let u1 = "file:/D:/IdeaProjects/new-ypyh/growup100-api/target/classes/";
        let u2 = "file:/D:/IdeaProjects/new-ypyh/growup100-business/target/classes/";
        let u3 = "file:/D:/apache-maven-3.9.0-bin/repository/org/example/lib/1.0/lib-1.0.jar";
        let mf = String::from_utf8(build_manifest(&[
            u1.into(),
            u2.into(),
            u3.into(),
        ]))
        .unwrap();
        let unfolded = unfold_classpath_attr(&mf);
        assert!(
            unfolded.contains(&format!("{u1} {u2}")),
            "urls must stay space-separated after unfold, got:\n{unfolded}"
        );
        assert!(
            !unfolded.contains("classes/file:"),
            "must not glue directory URL to next entry:\n{unfolded}"
        );
    }

    #[test]
    fn writes_classpath_jar() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let dir = std::env::temp_dir().join(format!("pl_cp_jar_{stamp}"));
        let _ = fs::remove_dir_all(&dir);
        let classes = dir.join("target/classes");
        fs::create_dir_all(&classes).unwrap();
        let jar = dir.join("target/.launcher-classpath.jar");
        write_classpath_jar(&jar, &[classes.clone()]).unwrap();
        assert!(jar.is_file());
        let padded = format!("{};{}", classes.display(), "Z".repeat(6500));
        let short = shorten_classpath_arg(&dir, &padded, &|_, _| {}).unwrap();
        assert!(short.contains(".launcher-classpath.jar"));
        let _ = fs::remove_dir_all(&dir);
    }
}
