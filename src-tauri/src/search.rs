//! Project-wide search (files / text / Spring endpoints) for Search Everywhere.
//! Heavy work runs on `spawn_blocking` so the UI / IPC thread stays responsive.

use crate::file_io::{project_root, resolve_under_root, should_skip_dir_name};
use crate::store::{find_project, load_projects};
use regex::Regex;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::AppHandle;
use walkdir::WalkDir;

const DEFAULT_LIMIT: usize = 50;
const MAX_WALK_FILES: usize = 20_000;
const MAX_TEXT_FILE_BYTES: u64 = 512 * 1024;

const TEXT_EXTS: &[&str] = &[
    "java", "kt", "vue", "wpy", "ts", "tsx", "js", "jsx", "mjs", "cjs", "xml", "yml", "yaml",
    "properties", "md", "html", "htm", "css", "scss", "less", "json", "sql", "rs", "txt", "gradle",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub kind: String,
    pub path: String,
    pub name: String,
    pub score: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module_hint: Option<String>,
}

fn want_kind(kinds: &Option<Vec<String>>, k: &str) -> bool {
    match kinds {
        None => true,
        Some(v) if v.is_empty() => true,
        Some(v) => v.iter().any(|x| x.eq_ignore_ascii_case(k)),
    }
}

fn to_slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn module_hint(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    rel.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
}

/// Substring score only (no subsequence) — keeps large repos usable.
fn path_score(hay: &str, needle_lower: &str) -> Option<i32> {
    if needle_lower.is_empty() {
        return Some(0);
    }
    let h = hay.to_lowercase();
    let pos = h.find(needle_lower)?;
    let mut score = 1000 - (pos as i32).min(900);
    if let Some(name) = Path::new(hay).file_name().and_then(|f| f.to_str()) {
        let nl = name.to_lowercase();
        if nl == needle_lower {
            score += 400;
        } else if nl.contains(needle_lower) {
            score += 200;
        }
    }
    Some(score)
}

fn ext_ok(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| TEXT_EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

fn walk_files<'a>(root: &'a Path, base: &'a Path) -> impl Iterator<Item = PathBuf> + 'a {
    WalkDir::new(base)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                !should_skip_dir_name(&name)
            } else {
                true
            }
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .filter(move |p| p.starts_with(root))
}

fn mapping_res() -> &'static (Regex, Regex, Regex) {
    static RE: OnceLock<(Regex, Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        (
            Regex::new(
                r#"(?m)@(?:RequestMapping|GetMapping|PostMapping|PutMapping|DeleteMapping|PatchMapping)\s*(?:\(([^)]*)\))?"#,
            )
            .expect("anno"),
            Regex::new(r#"(?:value|path)\s*=\s*"([^"]+)""#).expect("named"),
            Regex::new(r#"^\s*"([^"]+)"\s*$"#).expect("plain"),
        )
    })
}

fn method_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?s)(?:public|protected|private)?\s+(?:static\s+)?[\w.<>,\[\]\s]+\s+(\w+)\s*\("#,
        )
        .expect("method")
    })
}

fn http_method_from_anno(anno: &str) -> &'static str {
    if anno.contains("GetMapping") {
        "GET"
    } else if anno.contains("PostMapping") {
        "POST"
    } else if anno.contains("PutMapping") {
        "PUT"
    } else if anno.contains("DeleteMapping") {
        "DELETE"
    } else if anno.contains("PatchMapping") {
        "PATCH"
    } else {
        "REQUEST"
    }
}

fn extract_path_arg(args: &str) -> Option<String> {
    let args = args.trim();
    if args.is_empty() {
        return Some(String::new());
    }
    let (_anno, named, plain) = mapping_res();
    if let Some(c) = named.captures(args) {
        return Some(c[1].to_string());
    }
    if let Some(c) = plain.captures(args) {
        return Some(c[1].to_string());
    }
    if let Some(start) = args.find('"') {
        let rest = &args[start + 1..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

fn join_paths(class_path: &str, method_path: &str) -> String {
    let a = class_path.trim().trim_end_matches('/');
    let b = method_path.trim();
    if a.is_empty() {
        if b.starts_with('/') {
            b.to_string()
        } else if b.is_empty() {
            "/".into()
        } else {
            format!("/{b}")
        }
    } else if b.is_empty() {
        if a.starts_with('/') {
            a.to_string()
        } else {
            format!("/{a}")
        }
    } else {
        let b = b.trim_start_matches('/');
        let a = if a.starts_with('/') {
            a.to_string()
        } else {
            format!("/{a}")
        };
        format!("{a}/{b}")
    }
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn method_name_near(src: &str, anno_end: usize) -> Option<String> {
    let start = floor_char_boundary(src, anno_end.min(src.len()));
    method_name_re()
        .captures(&src[start..])
        .map(|c| c[1].to_string())
}

fn scan_java_endpoints(
    root: &Path,
    file: &Path,
    q_lower: &str,
    out: &mut Vec<SearchHit>,
    limit: usize,
    ep_count: &mut usize,
) {
    if *ep_count >= limit {
        return;
    }
    let Ok(meta) = fs::metadata(file) else {
        return;
    };
    if meta.len() > MAX_TEXT_FILE_BYTES * 2 {
        return;
    }
    let Ok(src) = fs::read_to_string(file) else {
        return;
    };
    // Cheap reject before regex
    if !src.contains("Mapping") {
        return;
    }
    let (anno_re, _, _) = mapping_res();

    let mut class_base = String::new();
    let mut class_level_done = false;
    for cap in anno_re.captures_iter(&src) {
        if *ep_count >= limit {
            break;
        }
        let whole = cap.get(0).map(|m| m.as_str()).unwrap_or("");
        let args = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let path_part = extract_path_arg(args).unwrap_or_default();
        let method = http_method_from_anno(whole);
        let end = floor_char_boundary(&src, cap.get(0).map(|m| m.end()).unwrap_or(0));
        let line = src[..end].bytes().filter(|&b| b == b'\n').count() as u32 + 1;

        let win_end = floor_char_boundary(&src, end.saturating_add(200).min(src.len()));
        let window = &src[end..win_end];
        let is_class = window.contains("class ") && !window.contains("(");

        if (is_class || (!class_level_done && whole.contains("RequestMapping") && window.contains("class ")))
            && !class_level_done
        {
            class_base = path_part;
            class_level_done = true;
            continue;
        }

        let full = join_paths(&class_base, &path_part);
        let name = file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        let score = path_score(&full, q_lower).or_else(|| path_score(&name, q_lower));
        let Some(score) = score else {
            continue;
        };
        let handler = method_name_near(&src, end).unwrap_or_else(|| "handler".into());
        out.push(SearchHit {
            kind: "endpoint".into(),
            path: to_slash(file),
            name: full.clone(),
            score,
            line: Some(line),
            preview: Some(format!("{full}  ({name}) [{method}]")),
            http_method: Some(method.into()),
            handler_method: Some(handler),
            module_hint: module_hint(root, file),
        });
        *ep_count += 1;
    }
}

fn run_search(
    root_canon: PathBuf,
    base: PathBuf,
    q: String,
    kinds: Option<Vec<String>>,
    limit: usize,
) -> Vec<SearchHit> {
    let q_lower = q.to_lowercase();
    let do_file = want_kind(&kinds, "file");
    let do_text = want_kind(&kinds, "text");
    let do_ep = want_kind(&kinds, "endpoint");

    let mut hits: Vec<SearchHit> = Vec::new();
    let mut file_count = 0usize;
    let mut text_count = 0usize;
    let mut ep_count = 0usize;
    let mut walked = 0usize;

    for file in walk_files(&root_canon, &base) {
        walked += 1;
        if walked > MAX_WALK_FILES {
            break;
        }
        // All requested buckets full → stop walking
        let files_done = !do_file || file_count >= limit;
        let text_done = !do_text || text_count >= limit;
        let ep_done = !do_ep || ep_count >= limit;
        if files_done && text_done && ep_done {
            break;
        }

        let rel = to_slash(&file);
        let name = file
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if do_file && file_count < limit {
            if let Some(score) = path_score(&rel, &q_lower).or_else(|| path_score(&name, &q_lower)) {
                hits.push(SearchHit {
                    kind: "file".into(),
                    path: rel.clone(),
                    name: name.clone(),
                    score,
                    line: None,
                    preview: None,
                    http_method: None,
                    handler_method: None,
                    module_hint: module_hint(&root_canon, &file),
                });
                file_count += 1;
            }
        }

        if do_ep && !ep_done && name.ends_with(".java") {
            scan_java_endpoints(
                &root_canon,
                &file,
                &q_lower,
                &mut hits,
                limit,
                &mut ep_count,
            );
        }

        if do_text && text_count < limit && ext_ok(&file) {
            if let Ok(meta) = fs::metadata(&file) {
                if meta.len() <= MAX_TEXT_FILE_BYTES {
                    if let Ok(content) = fs::read_to_string(&file) {
                        // Fast path: skip files without the needle (avoid full lowercasing when possible)
                        let has = content.contains(&q)
                            || (q != q_lower && content.to_lowercase().contains(&q_lower));
                        if has {
                            for (idx, line) in content.lines().enumerate() {
                                if text_count >= limit {
                                    break;
                                }
                                if line.to_lowercase().contains(&q_lower) {
                                    let preview: String = line.trim().chars().take(200).collect();
                                    hits.push(SearchHit {
                                        kind: "text".into(),
                                        path: rel.clone(),
                                        name: name.clone(),
                                        score: 500,
                                        line: Some((idx + 1) as u32),
                                        preview: Some(preview),
                                        http_method: None,
                                        handler_method: None,
                                        module_hint: module_hint(&root_canon, &file),
                                    });
                                    text_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.unwrap_or(0).cmp(&b.line.unwrap_or(0)))
    });

    if hits.len() > limit * 3 {
        hits.truncate(limit * 3);
    }
    hits
}

/// Async so the WebView stays interactive; search runs on a blocking pool thread.
#[tauri::command]
pub async fn search_everywhere(
    app: AppHandle,
    project_id: String,
    query: String,
    kinds: Option<Vec<String>>,
    base_path: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SearchHit>, String> {
    let q = query.trim().to_string();
    if q.is_empty() {
        return Ok(vec![]);
    }
    // Avoid accidental full-repo crawl on 1-char queries (still allow, but warn via speed)
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 200);
    let root = project_root(&app, &project_id)?;
    let root_canon = root
        .canonicalize()
        .map_err(|e| format!("project root invalid: {e}"))?;

    let base = if let Some(bp) = base_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        resolve_under_root(&root, bp)?
    } else {
        root_canon.clone()
    };
    if !base.is_dir() {
        return Err("base path is not a directory".into());
    }

    let _ = find_project(&load_projects(&app)?, &project_id);

    let kinds_bg = kinds;
    tauri::async_runtime::spawn_blocking(move || {
        run_search(root_canon, base, q, kinds_bg, limit)
    })
    .await
    .map_err(|e| format!("search task failed: {e}"))
}
