use super::archive::ArchiveHandle;
use super::file::{ASSET_EXTENSIONS, LIQUID_EXTENSIONS, MARKDOWN_EXTENSIONS};
use super::lru::LruCache;
use std::collections::HashMap;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// ANSI color escape sequences for the watcher's pretty logs (disabled
/// automatically when stderr is not an interactive terminal).
const LOG_STYLE_DIM: &str = "\u{1b}[2m";
const LOG_STYLE_GREEN: &str = "\u{1b}[32m";
const LOG_STYLE_YELLOW: &str = "\u{1b}[33m";
const LOG_STYLE_RED: &str = "\u{1b}[31m";
const LOG_STYLE_RESET: &str = "\u{1b}[0m";

/// `true` when the watcher's pretty logs may use ANSI colors — stderr
/// must be an interactive terminal. Captured/piped output (CI logs, test
/// runners) stays plain.
fn log_use_color() -> bool {
    io::stderr().is_terminal()
}

/// Build one pretty watcher line: `<label> <path>` with the label
/// colored by change kind (when `color` is on). Paths are shown relative
/// to `root` when possible, falling back to the absolute path for files
/// outside it.
fn format_change_line(kind: &str, path: &Path, root: &Path, color: bool) -> String {
    let (label, style) = match kind {
        "create" => ("created", LOG_STYLE_GREEN),
        "modify" => ("modified", LOG_STYLE_YELLOW),
        "remove" => ("removed", LOG_STYLE_RED),
        _ => ("changed", LOG_STYLE_DIM),
    };
    let rel = pretty_path(path, root);
    if color {
        format!("{style}{label}{LOG_STYLE_RESET} {rel}")
    } else {
        format!("{label} {rel}")
    }
}

/// Print one pretty watcher line to stderr — see
/// [`BaganCache::start_watching`] for the format.
pub fn log_change_pretty(kind: &str, path: &Path, root: &Path) {
    eprintln!("{}", format_change_line(kind, path, root, log_use_color()));
}

/// Window within which consecutive events for the same path count as
/// one operation. Shell redirects, editors, and atomic saves emit
/// several events for a single save — only the first within this
/// window logs.
const LOG_DEDUPE_WINDOW: Duration = Duration::from_secs(1);

/// `true` when a path may ever enter the cache (and therefore deserves
/// a pretty log line): markdown and asset extensions are the same
/// allow-lists the cache admits content by, so editor side files
/// (`.tmp`, swap files, lock files, …) never log.
fn log_trackable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            let ext_l = ext.to_lowercase();
            MARKDOWN_EXTENSIONS.contains(&ext_l.as_str())
                || ASSET_EXTENSIONS.contains(&ext_l.as_str())
                || LIQUID_EXTENSIONS.contains(&ext_l.as_str())
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| name.to_lowercase().ends_with(".liquid.html"))
        })
        .unwrap_or(false)
}

/// Decide whether a watcher event gets a pretty log line: each path
/// logs at most once per [`LOG_DEDUPE_WINDOW`] (create/remove included —
/// renames and atomic saves fire several events per operation) and
/// only for files the cache could ever track. Events are delivered to
/// `onChange` subscribers either way — this only filters the stderr log.
pub fn should_log_change(
    path: &Path,
    last_log_at: &HashMap<PathBuf, Instant>,
    now: Instant,
) -> bool {
    log_trackable(path)
        && !last_log_at
            .get(path)
            .is_some_and(|at| now.duration_since(*at) < LOG_DEDUPE_WINDOW)
}

/// Classify a watcher event from the **actual filesystem state** rather
/// than the raw notify event kind. Editors and atomic saves surface as
/// rename events ("modify"/"other" on the notify side), so the raw kind
/// cannot be trusted for a correct `created` / `modified` / `removed`
/// label:
///
/// - the file **exists** and was **already known to the cache** →
///   `modify` (the entry is being updated in place);
/// - the file **exists** and is **unknown** → `create`;
/// - the file is **gone** → `remove`.
///
/// `known` here means present in the cache tiers — exactly the paths a
/// create is allowed to add.
pub fn classify_change(memory: &LruCache, archive: &ArchiveHandle, path: &Path) -> &'static str {
    let known = memory.get(path).is_some() || archive.read(path).is_some();
    if path.exists() {
        if known { "modify" } else { "create" }
    } else {
        "remove"
    }
}

/// Render `path` relative to `root` when it lives underneath it
/// (`/project/contents/docs/a.md` → `contents/docs/a.md`), otherwise as
/// the lossy absolute path.
fn pretty_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|rel| rel.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}
