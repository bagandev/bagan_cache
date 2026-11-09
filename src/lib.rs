//! Memory-first cache of a site's Markdown and asset files.
//!
//! [`BaganCache`] loads supported Markdown files whose front-matter layout
//! matches a theme template (plus optional assets) into an in-memory LRU
//! store, derives template-ready [`PageObject`] / [`AssetsObject`] values on
//! demand, and
//! — in the background — flushes content into a binary shard archive
//! under `.peisar-cache` (with a `map.json` search index). Disk is *not*
//! the primary store: reads hit memory first and only fall through to the
//! archive for entries the LRU has evicted.
//!
//! # JS usage (napi-rs surface)
//!
//! ```js
//! const cache = new BaganCache("contents", "public", {
//!   markdown: { fragment: true },
//!   archive: { shardBytes: 8 * 1024 * 1024, flushIntervalMs: 2000 },
//!   hosting: { provider: "GithubPages", repo: "site" },
//!   outDir: "out",
//!   liquidCollections: ["_pages", "about.html"],
//!   liquidRender: (file) => renderLiquid(file), // (file: string) => PageObject
//! });
//! cache.pages();                    // PageObject[] — markdown + Liquid
//!                                   //   pages, for Liquid themes
//! cache.getPage("docs/intro.md");   // single page object (or null)
//! cache.assets();                    // AssetsObject[]
//! cache.liquids();                  // PageObject[] — Liquid collection pages
//! cache.mapJson();                   // the map.json document (search index)
//! cache.archived();                  // paths flushed to the disk archive
//! cache.startWatchingJs();           // keep everything in sync with the FS
//! cache.dispose();                   // stop watching and flush the archive
//! ```
//!
//! While watching, every file operation logs one pretty line to stderr —
//! `created docs/intro.md` (ANSI-colored on terminals, path relative to
//! the project root). Duplicate events from one save coalesce, and
//! filesystem noise is not logged.
//!
//! # Rust usage
//!
//! ```no_run
//! use bagan_cache::{CachedContent, BaganCache, BaganCacheConfig};
//! use std::path::Path;
//!
//! # fn main() -> std::io::Result<()> {
//! let mut cache = BaganCache::with_config(BaganCacheConfig {
//!     entry_dir: Some("contents".into()),
//!     assets_dir: Some("contents/public".into()),
//!     ..BaganCacheConfig::default()
//! })?;
//! for path in cache.markdown_files() {
//!     if let Some(CachedContent::Text(md)) = cache.get(Path::new(&path)) {
//!         println!("{} ({} bytes)", path, md.len());
//!     }
//! }
//! cache.start_watching()?;
//! # Ok(())
//! # }
//! ```
//!
//! # Discovery rules
//!
//! - *Markdown*: files under the entry directory whose extension is `md`,
//!   `markdown`, `mdown`, `mkdn`, `mkd`, `mdwn`, `mkdown`, or `ron`; cached
//!   as UTF-8 text when their front-matter layout matches an HTML layout
//!   template in the theme.
//! - *Assets*: files under the assets directory with a known image, script,
//!   style, font, or audio/video extension. Binary types are cached as raw
//!   bytes; `js`, `css`, `map`, and `svg` are cached as UTF-8 text.
//! - *Liquid*: `.html` / `.liquid` files inside the configured
//!   `liquidCollections` (directories are flattened recursively, files are
//!   taken as-is; both are resolved against the entry directory and must
//!   live inside it). The files carry YAML front matter like markdown
//!   pages; the JS-side `liquidRender(file)` callback extracts it, renders
//!   the Liquid content to HTML, and returns the front-matter fields, so
//!   liquid pages are [`PageObject`](crate::PageObject) values just like
//!   markdown pages (same shape).
//! - *Entry index*: the entry directory must contain an `index.md` (any
//!   supported markdown extension) **or** a Liquid `index.html` whose
//!   front matter sets a `layout` value.
//!
//! # Tiering
//!
//! 1. **Memory (hot)** — an LRU bounded by `byteBudget`/`entryBudget` in
//!    `memory` config (or the constructor options). All reads start here.
//!
//! 2. **Disk archive (cold)** — a background worker buffers upserts and
//!    flushes binary shards under `.peisar-cache` when the pending content
//!    crosses `shardBytes`, on the `flushIntervalMs` tick, or on
//!    flush/dispose/drop. `map.json` maps every flushed path to its shard
//!    location and (for markdown) its parsed front matter.
//!
//! Entries evicted from memory stay readable via the archive
//! (`getText`/`getBinary` fall through), so the byte budget is a *hot set*
//! limit, not a correctness limit.
//!
//! # Threading and lifecycle
//!
//! The archive worker and file watcher run on background threads. JS
//! consumers call `dispose()` (or drop the cache in Rust) to stop the
//! watcher, flush pending archive writes, and join the workers. Rust
//! consumers may also call [`BaganCache::stop_watching`] for the same
//! graceful shutdown — e.g. after a `Ctrl+C` interrupt.

mod archive;
mod base_url;
mod def;
mod file;
mod lru;
mod obj_cache;
mod theme;

pub use archive::ArchiveHandle;
pub use base_url::{BaseDirInfo, HostingProvider};
use def::{
    DEFAULT_ARCHIVE_DIR_NAME, DEFAULT_BYTE_BUDGET, DEFAULT_FLUSH_INTERVAL_MS, DEFAULT_OUT_DIR,
    DEFAULT_SHARD_BYTES,
};
pub use lru::LruCache;
pub use obj_cache::{
    AssetsObject, ObjectCache, PageObject, asset_file_type, asset_out_file, asset_slug,
    extra_frontmatter_fields, file_name_no_ext, liquid_file_name_no_ext, liquid_out_file,
    liquid_slug, markdown_slug, page_out_file,
};
use theme::{BaganThemeConfig, ThemeDirs};

use file::{
    ASSET_BINARY_EXTENSIONS, ASSET_EXTENSIONS, LIQUID_EXTENSIONS, MARKDOWN_EXTENSIONS,
    collect_asset_files, collect_markdown_files, has_liquid_extension,
};
#[cfg(feature = "npm")]
use napi::bindgen_prelude::*;
#[cfg(feature = "npm")]
use napi::threadsafe_function::ThreadsafeFunction;
#[cfg(feature = "npm")]
use napi_derive::napi;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
#[cfg(feature = "npm")]
use peisar::plugin::Plugin;
use peisar::{
    PeisarOptions, RenderOptions,
    ast::{AstOptions, md_to_ast_with_hooks},
    get_options,
    plugin::{ParserHooks, RegisteredPlugin, visit_document_mut},
    render_document_html,
};

use peisar_yaml::frontmatter;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// JS-side Liquid renderer for the configured `liquidCollections`
/// (npm builds): a synchronous `liquidRender(file: string) =>
/// PageObject` callback. The JS side extracts the YAML front matter,
/// renders the Liquid content to HTML, and returns the page fields; the
/// cache keeps its own path-derived fields (fileName, slug,
/// outFilePath, inputFilePath) and `raw`. The callback result is
/// converted leniently (via `serde_json::Value`) so JS consumers may
/// return a partial object — `html` is the only required behavior,
/// anything else falls back to the raw front matter. Rendering must
/// happen on the JS main thread — the same constraint as markdown
/// visitor plugins — so the callback is stored as a `FunctionRef` and
/// borrowed at render time (see
/// [`RegisteredPlugin`](peisar::plugin::RegisteredPlugin) for the same
/// pattern).
#[cfg(feature = "npm")]
pub type LiquidRenderer = FunctionRef<String, serde_json::Value>;

/// Rust-side Liquid renderer for the configured `liquidCollections`
/// (non-npm builds): takes the raw file text and returns the
/// front-matter fields + rendered HTML as a [`PageObject`].
#[cfg(not(feature = "npm"))]
pub type LiquidRenderer = Box<dyn Fn(String) -> PageObject + Send + Sync + 'static>;

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
fn log_change_pretty(kind: &str, path: &Path, root: &Path) {
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
fn should_log_change(path: &Path, last_log_at: &HashMap<PathBuf, Instant>, now: Instant) -> bool {
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
fn classify_change(memory: &LruCache, archive: &ArchiveHandle, path: &Path) -> &'static str {
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

/// A file-change event delivered to change callbacks
/// (see [`BaganCache::on_change`]).
#[cfg_attr(feature = "npm", napi(object))]
#[derive(Debug, Clone)]
pub struct CacheChangeEvent {
    /// Absolute path of the changed file.
    pub path: String,
    /// `"create"`, `"modify"`, `"remove"`, or `"other"`.
    pub kind: String,
    /// `true` when the file is under the markdown entry directory.
    pub is_markdown: bool,
}

/// An entry stored in the cache: raw UTF-8 text (markdown and textual
/// assets) or raw bytes (binary assets). Exposed to JavaScript so JS
/// consumers can branch on the variant. The serde derives back the
/// bincode round-trip through the disk archive.
#[cfg_attr(feature = "npm", napi)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum CachedContent {
    /// UTF-8 text content (markdown files and textual assets).
    Text(String),
    /// Raw bytes (binary assets like images or fonts).
    Binary(Vec<u8>),
}

/// Cache-wide configuration: directories, markdown options, memory/archive
/// budgets, hosting/baseUrl resolution, and the build output directory.
#[cfg_attr(feature = "npm", napi(object))]
#[derive(Debug, Clone, Default)]
pub struct BaganCacheConfig {
    /// Entry directory holding the markdown files (absolute, or relative
    /// to the current working directory). Defaults to the current directory.
    pub entry_dir: Option<String>,
    /// Optional assets directory, resolved from the current working
    /// directory and required to be inside `entry_dir`.
    pub assets_dir: Option<String>,
    /// Theme configuration object (`themeConfig`), imported directly on
    /// the JS side (e.g. the theme package's default export). When
    /// omitted, the theme directories below are resolved from the entry
    /// directory instead.
    pub theme_config: Option<BaganThemeConfig>,
    /// Theme layouts directory (used when `themeConfig` is omitted),
    /// resolved from the entry directory. Defaults to `_layouts`.
    pub theme_layouts_dir: Option<String>,
    /// Theme includes directory (used when `themeConfig` is omitted),
    /// resolved from the entry directory. Defaults to `_includes`.
    pub theme_includes_dir: Option<String>,
    /// Theme assets directory (used when `themeConfig` is omitted),
    /// resolved from the entry directory. Defaults to `_assets`.
    pub theme_assets_dir: Option<String>,
    /// Theme data directory (used when `themeConfig` is omitted),
    /// resolved from the entry directory. Defaults to `_data`.
    pub theme_data_dir: Option<String>,
    /// Markdown parsing/rendering options applied to every page render
    /// (GFM/Kramdown toggles, `fragment`, title, …). Custom visitors and
    /// parser hooks are registered separately via `usePlugin()`.
    pub markdown: Option<PeisarOptions>,
    /// Memory-tier budgets (LRU eviction). Defaults: 64 MiB bytes,
    /// unlimited entries.
    pub memory: Option<MemoryConfig>,
    /// Disk-archive tier thresholds. Defaults: 8 MiB shards, 2 s flush.
    pub archive: Option<ArchiveConfig>,
    /// Static-hosting baseUrl resolution (GitHub Pages/Vercel/…).
    pub hosting: Option<HostingConfig>,
    /// Build output directory used for `outFilePath` fields (default
    /// `".bagan"`).
    pub out_dir: Option<String>,
    /// Archive directory, relative to the current working directory
    /// (default `".peisar-cache"`).
    pub cache_dir: Option<String>,
    /// Liquid (`.html`) collections inside the entry directory: each
    /// entry is a directory (flattened recursively) or a single file,
    /// resolved against the entry directory. The discovered files are
    /// cached as text, watched for changes, and exposed as
    /// [`PageObject`](crate::PageObject) values through `pages()` /
    /// `liquids()`. Entries outside the entry
    /// directory are rejected at construction.
    pub liquid_collections: Option<Vec<String>>,
}

/// LRU budgets for the memory tier.
#[cfg_attr(feature = "npm", napi(object))]
#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryConfig {
    /// Maximum total bytes of raw content kept in memory (`0` =
    /// unlimited). When the budget is exceeded, the least recently used
    /// entries are evicted (they remain readable from the disk archive).
    pub byte_budget: Option<i64>,
    /// Maximum number of entries in memory (`0` = unlimited).
    pub entry_budget: Option<u32>,
}

impl MemoryConfig {
    /// Resolved byte budget (`0` = unlimited).
    pub fn byte_budget(&self) -> u64 {
        self.byte_budget
            .unwrap_or(DEFAULT_BYTE_BUDGET as i64)
            .max(0) as u64
    }

    /// Resolved entry budget (`0` = unlimited).
    pub fn entry_budget(&self) -> usize {
        self.entry_budget.unwrap_or(0) as usize
    }
}

/// Flush thresholds for the disk-archive tier.
#[cfg_attr(feature = "npm", napi(object))]
#[derive(Debug, Clone, Copy, Default)]
pub struct ArchiveConfig {
    /// Pending-content threshold (bytes) that triggers writing a new
    /// shard; `0` disables size-based flushing (interval/drop only).
    pub shard_bytes: Option<i64>,
    /// Maximum time (ms) pending entries may sit in the worker buffer
    /// before being flushed. `0` disables the timer.
    pub flush_interval_ms: Option<i64>,
}

impl ArchiveConfig {
    /// Resolved shard threshold.
    pub fn shard_bytes(&self) -> u64 {
        self.shard_bytes
            .unwrap_or(DEFAULT_SHARD_BYTES as i64)
            .max(0) as u64
    }

    /// Resolved flush interval.
    pub fn flush_interval(&self) -> Duration {
        Duration::from_millis(
            self.flush_interval_ms
                .unwrap_or(DEFAULT_FLUSH_INTERVAL_MS as i64)
                .max(0) as u64,
        )
    }
}

/// Static-hosting configuration for baseUrl resolution.
#[cfg_attr(feature = "npm", napi(object))]
#[derive(Debug, Clone, Default)]
pub struct HostingConfig {
    /// Hosting provider (see [`HostingProvider`]).
    #[cfg_attr(feature = "npm", napi(ts_type = "BaganHostingProvider"))]
    pub provider: Option<HostingProvider>,
    /// GitHub repository name for project pages (`/<repo>/` prefix).
    pub repo: Option<String>,
    /// Custom site URL (overrides provider defaults when set).
    pub site_url: Option<String>,
    /// Extra path prefix (e.g. `"subpath"` for `example.com/subpath/`).
    pub path_prefix: Option<String>,
}

impl HostingConfig {
    fn resolve(&self) -> BaseDirInfo {
        let mut info = BaseDirInfo::new(
            self.provider.unwrap_or(HostingProvider::Custom),
            self.repo.clone(),
            self.site_url.clone(),
        );
        info.path_prefix = self.path_prefix.clone();
        info
    }
}

impl Default for BaseDirInfo {
    fn default() -> Self {
        Self::new(HostingProvider::Custom, None, None)
    }
}

/// Boxed change callback held in the subscription registry.
///
/// In npm builds, JavaScript callbacks are wrapped into a thread-safe
/// function before they are stored because watcher events fire off-thread.
pub type CacheChangeCallback = Box<dyn Fn(CacheChangeEvent) + Send + 'static>;

/// Event-loop keep-alive held while the watcher runs (npm builds): a
/// strong (ref'd) threadsafe function, so Node keeps running as long as
/// watching is active — even for a plain script with nothing else pending.
///
/// `CalleeHandled = false` matches what
/// [`Function::build_threadsafe_function`] produces.
#[cfg(feature = "npm")]
pub type KeepAlive = ThreadsafeFunction<(), Unknown<'static>, (), Status, false>;

/// Backward-compatible name for callbacks used by the JavaScript binding.
pub type JsChangeCallback = CacheChangeCallback;

/// Shared registry of change callbacks, keyed by subscription id.
#[derive(Clone, Default)]
struct ChangeCallbacks {
    inner: Arc<Mutex<HashMap<u32, CacheChangeCallback>>>,
    next_id: Arc<AtomicU32>,
}

impl ChangeCallbacks {
    fn add(&self, cb: CacheChangeCallback) -> u32 {
        let mut map = self.inner.lock().unwrap();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        map.insert(id, cb);
        id
    }

    fn remove(&self, id: u32) {
        self.inner.lock().unwrap().remove(&id);
    }

    fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }

    fn dispatch(&self, event: CacheChangeEvent) {
        let map = self.inner.lock().unwrap();
        for cb in map.values() {
            cb(event.clone());
        }
    }
}

/// In-memory + archive-backed cache of the Markdown and asset files under a
/// directory tree.
///
/// Memory is the source of truth for hot content; a background worker
/// flushes a binary shard archive to `.peisar-cache` for cold/large
/// entries, front-ended by the `map.json` search index. Watchers keep the
/// memory tier in sync with the filesystem.
#[cfg_attr(feature = "npm", napi)]
pub struct BaganCache {
    /// LRU memory tier: absolute source path → raw content. Clone-shared
    /// with the watcher thread.
    memory: LruCache,
    /// Disk-archive tier handle (shards + map.json index). Clone-shared
    /// with the watcher thread.
    archive: ArchiveHandle,
    plugins: Arc<Mutex<Vec<RegisteredPlugin>>>,
    /// Resolved markdown AST options (GFM/Kramdown, file name).
    ast_opts: AstOptions,
    /// Resolved markdown render options (fragment, title, …).
    render_opts: RenderOptions,
    watcher: Option<RecommendedWatcher>,
    /// Keep-alive handle for the Node event loop while watching is
    /// active (npm builds only): a strong (ref'd) threadsafe function
    /// over a no-op JS callback. The native `notify` watcher thread is
    /// invisible to libuv, so without this a plain script like
    /// `cache.startWatchingJs()` would exit the moment the script ends.
    /// `stop_watching` / `dispose` / dropping the cache drop the handle,
    /// which lets the process exit again.
    #[cfg(feature = "npm")]
    watch_keep_alive: Option<KeepAlive>,
    entry_dir: PathBuf,
    assets_dir: Option<PathBuf>,
    theme_config: BaganThemeConfig,
    theme_assets_dir: PathBuf,
    theme_layouts_dir: PathBuf,
    theme_data_dir: PathBuf,
    theme_includes_dir: PathBuf,
    markdown_layouts: HashSet<String>,
    change_callbacks: ChangeCallbacks,
    /// Hosting/baseUrl configuration.
    hosting: BaseDirInfo,
    /// Build output directory (for `outFilePath` fields).
    out_dir: String,
    /// Liquid (`.html`) collections: tracked absolute files mapped to
    /// their slug base directory (the collection root for directories,
    /// the entry directory for single files). Clone-shared with the
    /// watcher thread.
    liquid_files: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
    /// Configured liquid collection roots (the raw resolved
    /// directories/files from `liquidCollections`), used to re-flatten
    /// after directory-level events. Clone-shared with the watcher thread.
    liquid_collection_roots: Arc<Mutex<Vec<PathBuf>>>,
    /// JS-side `liquidRender(file)` callback for liquid page objects
    /// (npm builds hold a `FunctionRef` + the napi `Env` captured at
    /// construction; rendering runs on the JS main thread only). The
    /// callback returns the front-matter fields + rendered HTML.
    #[cfg(feature = "npm")]
    liquid_renderer: Option<LiquidRenderer>,
    /// Rust-side Liquid renderer (non-npm builds).
    #[cfg(not(feature = "npm"))]
    liquid_renderer: Option<LiquidRenderer>,
    /// The napi `Env` captured at construction, used to `borrow_back` the
    /// `liquidRenderer` callback (npm builds only).
    #[cfg(feature = "npm")]
    liquid_env: Option<Env>,
}

/// Build the event-loop keep-alive held while the watcher runs: a strong
/// (ref'd, i.e. NOT `.weak()`) threadsafe function over a no-op JS
/// function. The ref'd TSFN is visible to libuv, so Node keeps the
/// process alive as long as the handle exists; dropping it unrefs the
/// loop again and lets the process exit.
#[cfg(feature = "npm")]
fn build_keep_alive(env: &napi::Env) -> Result<KeepAlive> {
    // `()` is `Undefined`: the no-op callback takes no arguments and
    // returns nothing, it is never actually invoked. The default (strong)
    // TSFN — no `.weak()` call — keeps the loop alive until the handle
    // is dropped.
    env.create_function::<(), Unknown<'static>>("peisar_noop", noop)?
        .build_threadsafe_function::<()>()
        .build()
}

/// The no-op JS callback backing the event-loop keep-alive. The return
/// value must be a valid `napi_value`, so it returns `undefined`.
#[cfg(feature = "npm")]
unsafe extern "C" fn noop(
    env: napi::sys::napi_env,
    _info: napi::sys::napi_callback_info,
) -> napi::sys::napi_value {
    let mut undefined = std::ptr::null_mut();
    unsafe {
        napi::sys::napi_get_undefined(env, &mut undefined);
    }
    undefined
}

/// Render one markdown document into HTML using the resolved options and
/// the registered visitors/parsers (if any).
fn render_markdown(
    raw_md: &str,
    ast_opts: &AstOptions,
    render_opts: &RenderOptions,
    plugins: &Arc<Mutex<Vec<RegisteredPlugin>>>,
    file_name: Option<String>,
) -> String {
    // Front matter never reaches the renderer.
    let (md_body, _fm) = match frontmatter(raw_md.to_string()) {
        Ok(parsed) => parsed.into_parts(),
        Err(_) => (raw_md.to_string(), None),
    };
    let mut hooks_lock = plugins.lock().unwrap();
    let opts = AstOptions {
        gfm: ast_opts.gfm,
        kramdown: ast_opts.kramdown,
        file_name,
    };
    let mut doc = {
        let mut hooks = ParserHooks::empty();
        for p in hooks_lock.iter() {
            hooks.push(p);
        }
        md_to_ast_with_hooks(&md_body, &opts, opts.file_name.clone(), &hooks)
    };
    for p in hooks_lock.iter_mut() {
        visit_document_mut(&mut doc, p);
    }
    render_document_html(doc, Some(render_opts.clone()))
}

/// Extract a page title: front matter `title` first, else the first `#`
/// heading before any other block content.
fn extract_title(fm: Option<&serde_json::Value>, md_body: &str) -> Option<String> {
    if let Some(title) = fm.and_then(|fm| fm.get("title")).and_then(|t| t.as_str()) {
        return Some(title.to_string());
    }
    for line in md_body.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        } else if !trimmed.is_empty() {
            // First non-empty, non-heading line ends the search.
            return None;
        }
    }
    None
}

/// Front-matter field helpers (string / string-array flattening).
fn fm_string(fm: Option<&serde_json::Value>, keys: &[&str]) -> Option<String> {
    let fm = fm?;
    for key in keys {
        if let Some(v) = fm.get(*key) {
            return Some(match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            });
        }
    }
    None
}

fn fm_string_list(fm: Option<&serde_json::Value>, key: &str) -> Option<Vec<String>> {
    let fm = fm?;
    let arr = fm.get(key)?.as_array()?;
    Some(
        arr.iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
    )
}

fn markdown_file_for_layouts(path: &Path, layouts: &HashSet<String>) -> io::Result<Option<String>> {
    let text = fs::read_to_string(path)?;
    let parsed = frontmatter(text.clone()).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "failed to parse front matter for '{}': {error}",
                path.display()
            ),
        )
    })?;
    let matches_layout = parsed
        .yaml_data()
        .and_then(|metadata| metadata.get("layout"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|layout| layouts.contains(layout));
    Ok(matches_layout.then_some(text))
}

/// `true` when a Liquid `index.html` file carries a front-matter
/// `layout` value: such a file is a valid entry-page alternative to
/// `index.{markdownExts}`.
fn liquid_index_has_layout(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    frontmatter(text)
        .ok()
        .and_then(|parsed| parsed.yaml_data().cloned())
        .and_then(|fm| fm.get("layout").cloned())
        .is_some_and(|layout| !layout.is_null())
}

fn collect_markdown_content(
    entry_dir: &Path,
    layouts: &HashSet<String>,
) -> io::Result<HashMap<PathBuf, String>> {
    let mut markdown = HashMap::new();
    for path in collect_markdown_files(entry_dir)? {
        if let Some(text) = markdown_file_for_layouts(&path, layouts)? {
            markdown.insert(path, text);
        }
    }
    Ok(markdown)
}

fn collect_asset_content(assets_dir: &Path) -> io::Result<HashMap<PathBuf, CachedContent>> {
    let mut assets = HashMap::new();
    for path in collect_asset_files(assets_dir)? {
        let is_binary = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ASSET_BINARY_EXTENSIONS.contains(&ext.to_lowercase().as_str()));
        let content = if is_binary {
            CachedContent::Binary(fs::read(&path)?)
        } else {
            CachedContent::Text(fs::read_to_string(&path)?)
        };
        assets.insert(path, content);
    }
    Ok(assets)
}

/// Base64 encode bytes (standard alphabet with padding) without a
/// dependency — used for `AssetsObject.raw` of binary assets.
fn binary_to_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 63) as usize] as char);
        out.push(TABLE[((triple >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
fn layout_names(dir: PathBuf) -> HashSet<String> {
    let mut results: HashSet<String> = HashSet::new();

    if dir.is_dir() {
        // Use if let instead of unwrap to gracefully skip unreadable directories
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                // .flatten() safely skips error entries (e.g. permission issues)
                let path = entry.path();

                if path.is_dir() {
                    let sub_names = layout_names(path);
                    results.extend(sub_names);
                } else if path.extension().and_then(|s| s.to_str()) == Some("html") {
                    // Use file_stem safely without unwrap
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        results.insert(stem.to_string());
                    }
                }
            }
        }
    }
    results
}

#[cfg_attr(feature = "npm", napi)]
impl BaganCache {
    /// JS: `new BaganCache(entryDir?, assetsDir?, options?)`.
    ///
    /// `options` configures the theme directories (or a `themeConfig`
    /// object imported on the JS side), Markdown parsing (`fragment`,
    /// GFM, …), archive thresholds, hosting resolution, and output
    /// directories. Register custom visitors and parser hooks afterwards
    /// with `usePlugin()`.
    #[cfg(feature = "npm")]
    #[napi(constructor)]
    pub fn new(
        env: Env,
        entry_dir: Option<String>,
        assets_dir: Option<String>,
        options: Option<BaganCacheJsOptions>,
    ) -> Result<Self> {
        let config = BaganCacheConfig {
            entry_dir,
            assets_dir,
            theme_config: options.as_ref().and_then(|o| o.theme_config.clone()),
            theme_layouts_dir: options.as_ref().and_then(|o| o.theme_layouts_dir.clone()),
            theme_includes_dir: options.as_ref().and_then(|o| o.theme_includes_dir.clone()),
            theme_assets_dir: options.as_ref().and_then(|o| o.theme_assets_dir.clone()),
            theme_data_dir: options.as_ref().and_then(|o| o.theme_data_dir.clone()),
            markdown: options.as_ref().and_then(|o| o.markdown.clone()),
            archive: options.as_ref().and_then(|o| o.archive),
            hosting: options.as_ref().and_then(|o| o.hosting.clone()),
            out_dir: options.as_ref().and_then(|o| o.out_dir.clone()),
            cache_dir: options.as_ref().and_then(|o| o.cache_dir.clone()),
            liquid_collections: options.as_ref().and_then(|o| o.liquid_collections.clone()),
            memory: None,
        };
        // JS-side `liquidRender(file)` callback: received as a `Function`
        // (napi objects cannot hold `FunctionRef` fields) and converted to
        // a `FunctionRef` for later main-thread invocation.
        let liquid_renderer: Option<LiquidRenderer> = options
            .as_ref()
            .and_then(|o| o.liquid_render.as_ref())
            .and_then(|render| render.create_ref().ok());
        BaganCache::with_config_liquid(config, liquid_renderer, Some(env))
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, format!("{e}")))
    }

    /// JS: `BaganCache.withConfigJs(config)` — full configuration form,
    /// including memory-tier budgets. Liquid collections may be
    /// configured through `liquidCollections`, but `liquidRender` is only
    /// available on the constructor (the `withConfig` object cannot carry
    /// functions across the boundary) — use `useLiquidRender()` to
    /// register the callback afterwards.
    #[cfg(feature = "npm")]
    #[napi(factory)]
    pub fn with_config_js(config: BaganCacheConfig) -> Result<Self> {
        BaganCache::with_config(config)
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, format!("{e}")))
    }

    /// Construct a cache from a full configuration (Rust entry point).
    ///
    /// Construction loads all markdown (and optionally assets) into the
    /// memory tier; disk writes happen later on the archive worker's
    /// size/interval flush (or explicitly via [`Self::flush`]). Liquid
    /// collections are resolved and loaded, but without a renderer the
    /// front-matter fields come from the raw front matter and `html`
    /// stays empty (Rust consumers can use [`Self::use_liquid_render`]).
    pub fn with_config(config: BaganCacheConfig) -> io::Result<Self> {
        #[cfg(feature = "npm")]
        {
            Self::with_config_liquid(config, None, None)
        }
        #[cfg(not(feature = "npm"))]
        {
            Self::with_config_liquid(config, None)
        }
    }

    /// [`Self::with_config`] with an optional Liquid renderer: the JS
    /// `liquidRender(file) => PageObject` callback (npm builds) or a
    /// boxed Rust closure (non-npm builds), invoked when liquid page
    /// objects are built.
    pub fn with_config_liquid(
        config: BaganCacheConfig,
        liquid_renderer: Option<LiquidRenderer>,
        #[cfg(feature = "npm")] liquid_env: Option<Env>,
    ) -> io::Result<Self> {
        let cwd = env::current_dir()?;
        // Resolve the entry dir to an absolute path first: watcher events
        // carry absolute paths, so the `is_markdown` prefix check needs an
        // absolute entry_dir even when JS passes a relative one — and the
        // theme directories below resolve from it when `themeConfig` is
        // omitted.
        let entry_dir = {
            let d = Path::new(config.entry_dir.as_deref().unwrap_or("."));
            if d.is_absolute() {
                d.to_path_buf()
            } else {
                cwd.join(d)
            }
        };
        // The entry directory must have an index page: either a markdown
        // file (`index.{markdownExts}`) or a Liquid `index.html` whose
        // front matter sets a `layout`.
        let has_markdown_index = MARKDOWN_EXTENSIONS
            .iter()
            .any(|ext| entry_dir.join(format!("index.{ext}")).is_file());
        let has_liquid_index = entry_dir.join("index.html").is_file()
            && liquid_index_has_layout(&entry_dir.join("index.html"));
        if !has_markdown_index && !has_liquid_index {
            let message = format!(
                "entry directory '{}' must contain an index file with a supported Markdown \
                 extension or a Liquid index.html with a front-matter layout",
                entry_dir.display()
            );
            eprintln!("BaganCache configuration error: {message}");
            return Err(io::Error::new(io::ErrorKind::NotFound, message));
        }
        let entry_dir = fs::canonicalize(&entry_dir)?;
        // Resolve the theme directories:
        // - `themeConfig` set → its directory names (or the defaults)
        //   resolve from the entry directory.
        // - `themeConfig` omitted → `theme_layouts_dir`/`theme_includes_dir`/
        //   `theme_assets_dir`/`theme_data_dir` (or the defaults
        //   `_layouts`/`_includes`/`_assets`/`_data`) resolve from the
        //   entry directory.
        let theme_config = match config.theme_config.clone() {
            Some(theme_config) => theme_config,
            None => BaganThemeConfig {
                layouts_dir: config.theme_layouts_dir.clone(),
                includes_dir: config.theme_includes_dir.clone(),
                assets_dir: config.theme_assets_dir.clone(),
                data_dir: config.theme_data_dir.clone(),
                ..Default::default()
            },
        };
        let theme_assets_dir = entry_dir.join(theme_config.assets_dir_or_default());
        let theme_layouts_dir = entry_dir.join(theme_config.layouts_dir_or_default());
        let theme_includes_dir = entry_dir.join(theme_config.includes_dir_or_default());
        let theme_data_dir = entry_dir.join(theme_config.data_dir_or_default());
        if theme_assets_dir.exists() && !theme_assets_dir.is_dir() {
            let message = format!(
                "theme assets path '{}' is not a directory",
                theme_assets_dir.display()
            );
            eprintln!("BaganCache configuration error: {message}");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        // Assets are resolved from cwd and must remain inside entry_dir.
        let resolved_assets: Option<PathBuf> = match &config.assets_dir {
            Some(p) => {
                let pbuf = PathBuf::from(p);
                let path = if pbuf.is_absolute() {
                    pbuf
                } else {
                    cwd.join(pbuf)
                };
                let resolved = match fs::canonicalize(&path) {
                    Ok(resolved) if resolved.is_dir() => resolved,
                    Ok(_) => {
                        let message =
                            format!("assets directory '{}' is not a directory", path.display());
                        eprintln!("BaganCache configuration error: {message}");
                        return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
                    }
                    Err(error) => {
                        let message = format!(
                            "assets directory '{}' does not exist or cannot be accessed: {error}",
                            path.display()
                        );
                        eprintln!("BaganCache configuration error: {message}");
                        return Err(io::Error::new(error.kind(), message));
                    }
                };
                if !resolved.starts_with(&entry_dir) {
                    let message = format!(
                        "assets directory '{}' must be inside entry directory '{}'",
                        resolved.display(),
                        entry_dir.display()
                    );
                    eprintln!("BaganCache configuration error: {message}");
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
                }
                Some(resolved)
            }
            None => None,
        };
        // Resolve the Liquid collections against the entry directory:
        // each entry may be a directory (recursively flattened) or a
        // single `.html`/`.liquid` file; the discovered files are merged
        // into one deduplicated map (file → slug base directory). Files
        // from a directory collection slug relative to the collection
        // root; single files slug relative to the entry directory.
        // Entries are resolved from the entry directory and must exist
        // and live inside it — anything else is a configuration error.
        // The Liquid entry index (entry-dir `index.html` with a
        // front-matter layout, when there is no markdown index) is always
        // tracked as a liquid page, slugged relative to the entry dir.
        let (liquid_files, liquid_roots): (HashMap<PathBuf, PathBuf>, Vec<PathBuf>) = {
            let mut roots: Vec<PathBuf> = Vec::new();
            let mut files: HashMap<PathBuf, PathBuf> = HashMap::new();
            // The Liquid entry index is always tracked as a liquid page
            // (and registered as a collection root so the watcher keeps
            // it in sync without an explicit liquidCollections entry).
            let liquid_index = entry_dir.join("index.html");
            if !has_markdown_index && has_liquid_index {
                files.insert(liquid_index.clone(), entry_dir.clone());
                roots.push(liquid_index);
            }
            for entry in config.liquid_collections.iter().flatten() {
                let trimmed = entry.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let candidate = entry_dir.join(trimmed);
                let resolved = match fs::canonicalize(&candidate) {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        let message = format!(
                            "liquid collection '{}' does not exist or cannot be accessed: {error}",
                            candidate.display()
                        );
                        eprintln!("BaganCache configuration error: {message}");
                        return Err(io::Error::new(error.kind(), message));
                    }
                };
                if !resolved.starts_with(&entry_dir) {
                    let message = format!(
                        "liquid collection '{}' must be inside entry directory '{}'",
                        resolved.display(),
                        entry_dir.display()
                    );
                    eprintln!("BaganCache configuration error: {message}");
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
                }
                roots.push(resolved.clone());
                if resolved.is_dir() {
                    for path in file::collect_liquid_files(&resolved) {
                        files.insert(path, resolved.clone());
                    }
                } else if has_liquid_extension(&resolved) {
                    files.insert(resolved, entry_dir.clone());
                } else {
                    let message = format!(
                        "liquid collection '{}' is neither a directory nor a Liquid file",
                        resolved.display()
                    );
                    eprintln!("BaganCache configuration error: {message}");
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
                }
            }
            (files, roots)
        };
        // Get layouts names. In local mode the layouts directory may not
        // exist yet — that only means no markdown page matches a layout,
        // which is a valid (empty) site, not a configuration error.
        if theme_layouts_dir.exists() && !theme_layouts_dir.is_dir() {
            let message = format!(
                "layouts dir path '{}' is not a directory",
                theme_layouts_dir.display()
            );
            eprintln!("BaganCache configuration error: {message}");
            return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
        }
        let markdown_layouts: HashSet<String> = layout_names(theme_layouts_dir.clone());

        // Markdown options for every page render.
        let resolved = get_options(config.markdown.clone());
        let ast_opts = resolved.ast_opts;
        let render_opts = resolved.render_opts;

        // Memory tier.
        let mem_cfg = config.memory.unwrap_or_default();
        let memory = LruCache::new(mem_cfg.byte_budget(), mem_cfg.entry_budget());

        let cache_dir = config
            .cache_dir
            .unwrap_or_else(|| DEFAULT_ARCHIVE_DIR_NAME.to_string());
        let out_dir = config
            .out_dir
            .unwrap_or_else(|| DEFAULT_OUT_DIR.to_string());

        // Archive tier.
        let archive_cfg = config.archive.unwrap_or_default();
        let archive = ArchiveHandle::new(
            cwd.clone(),
            archive_cfg.shard_bytes(),
            archive_cfg.flush_interval(),
            Some(cache_dir.clone()),
        );

        // Hosting resolution.
        let hosting = config
            .hosting
            .as_ref()
            .map(HostingConfig::resolve)
            .unwrap_or_default();

        // Discover and load markdown + assets (+ liquid) into the memory tier.
        let mut cache_map: HashMap<PathBuf, CachedContent> = HashMap::new();
        for (path, text) in collect_markdown_content(&entry_dir, &markdown_layouts)? {
            cache_map.insert(path, CachedContent::Text(text));
        }
        if let Some(ref a_dir) = resolved_assets {
            cache_map.extend(collect_asset_content(a_dir)?);
        }
        if theme_assets_dir.is_dir() {
            cache_map.extend(collect_asset_content(&theme_assets_dir)?);
        }
        for path in liquid_files.keys() {
            if !cache_map.contains_key(path)
                && let Ok(text) = fs::read_to_string(path)
            {
                cache_map.insert(path.clone(), CachedContent::Text(text));
            }
        }
        memory.replace_all(cache_map.clone());

        // Enqueue the initial snapshot for an immediate background flush,
        // independent of the configured interval.
        archive.sync_all_and_flush(cache_map);

        // Make sure the archive dir is ignored by git.
        archive::ensure_gitignore_entry(&cwd, Some(cache_dir.clone()), Some(out_dir.clone()))?;

        #[allow(clippy::arc_with_non_send_sync)]
        let plugins: Arc<Mutex<Vec<RegisteredPlugin>>> = Arc::new(Mutex::new(Vec::new()));
        Ok(BaganCache {
            memory,
            archive,
            plugins,
            ast_opts,
            render_opts,
            watcher: None,
            #[cfg(feature = "npm")]
            watch_keep_alive: None,
            entry_dir,
            assets_dir: resolved_assets,
            theme_config,
            theme_assets_dir,
            theme_layouts_dir,
            theme_data_dir,
            theme_includes_dir,
            markdown_layouts,
            change_callbacks: ChangeCallbacks::default(),
            hosting,
            out_dir,
            liquid_files: Arc::new(Mutex::new(liquid_files)),
            liquid_collection_roots: Arc::new(Mutex::new(liquid_roots)),
            liquid_renderer,
            #[cfg(feature = "npm")]
            liquid_env,
        })
    }

    /// Construct a [`BaganCache`] over `entry_dir` with the default
    /// configuration — Rust convenience (generic signatures cannot cross
    /// the NAPI boundary).
    #[cfg(not(feature = "npm"))]
    pub fn new<P: AsRef<Path>>(entry_dir: P) -> io::Result<Self> {
        BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some(entry_dir.as_ref().to_string_lossy().to_string()),
            ..Default::default()
        })
    }

    /// Register a JavaScript plugin applied to every page render
    /// (JS: `cache.usePlugin({ visitBlock, visitInline, parseBlock, parseInline })`).
    #[cfg(feature = "npm")]
    #[napi]
    pub fn use_plugin(&mut self, env: Env, plugin: Plugin) {
        self.plugins.lock().unwrap().push(plugin.register(env));
    }

    /// JS: `cache.useLiquidRender(cb)` — register the JavaScript function
    /// that renders a Liquid collection file to a page
    /// (`liquidRender(file: string) => PageObject`). The callback
    /// extracts the front matter, renders the Liquid content to HTML,
    /// and returns the page fields (converted leniently — `html` is the
    /// meaningful field, anything else falls back to the raw front
    /// matter); the cache fills the path-derived fields (fileName,
    /// slug, outFilePath, inputFilePath) and `raw` itself. Called on
    /// the JS main thread every time `pages()` / `liquids()` /
    /// `getLiquid()` builds a liquid page object; a callback that
    /// throws or returns nothing leaves the front-matter fields empty
    /// and `html` as the raw template. Also accepts the callback
    /// through the constructor option `liquidRender`.
    #[cfg(feature = "npm")]
    #[napi]
    pub fn use_liquid_render(
        &mut self,
        env: Env,
        #[napi(ts_arg_type = "(file: string) => PageObject")] render: Function<
            'static,
            String,
            serde_json::Value,
        >,
    ) {
        // The napi env is captured here too: `withConfigJs` consumers have
        // none stored from construction, and the FunctionRef needs it to
        // be borrowed back on the JS main thread.
        self.liquid_env = Some(env);
        self.liquid_renderer = render.create_ref().ok();
    }

    /// Register a Rust closure that renders Liquid collection files for
    /// `pages()` / `liquids()` / `get_liquid()` (non-npm builds): it
    /// receives the raw file text and returns the front-matter fields +
    /// rendered HTML as a [`PageObject`]. `withConfig`
    /// consumers cannot pass functions through the napi config object,
    /// so this is the Rust counterpart of the constructor's
    /// `liquidRender` option.
    #[cfg(not(feature = "npm"))]
    pub fn use_liquid_render<F>(&mut self, render: F)
    where
        F: Fn(String) -> PageObject + Send + Sync + 'static,
    {
        self.liquid_renderer = Some(Box::new(render));
    }
    /// JS: `cache.startWatchingJs()` — start watching the entry (and
    /// assets) directories recursively; errors surface as JS exceptions.
    ///
    /// While watching is active this holds a strong (ref'd) threadsafe
    /// function on the Node event loop, so a plain script that only calls
    /// `startWatchingJs()` keeps running until `dispose()` (or process
    /// exit) stops the watcher. Without it, the native watcher thread is
    /// invisible to libuv and Node exits as soon as the script ends.
    #[cfg(feature = "npm")]
    #[napi]
    pub fn start_watching_js(&mut self, env: Env) -> Result<()> {
        self.start_watching()
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, format!("{e}")))?;
        // Watcher started: take over the event loop. The no-op callback
        // keeps the (default strong) TSFN ref'd, which is the only thing
        // libuv sees of the otherwise native-side watcher thread.
        self.watch_keep_alive = Some(build_keep_alive(&env)?);
        Ok(())
    }

    /// Start watching the entry (and assets) directories recursively.
    ///
    /// From then on, create/modify/remove events update the memory tier
    /// first, then enqueue matching archive commands; directory events
    /// trigger a full rescan. Every operation also logs one pretty line
    /// to stderr — `created contents/docs/intro.md` — with the path
    /// relative to the project root. The label is derived from the
    /// actual filesystem state (editors surface atomic saves as rename
    /// events, so the raw event kind cannot be trusted), duplicate events
    /// for one save coalesce (an edit logs `modified` exactly once),
    /// and filesystem noise is not logged; `onChange` subscribers still
    /// receive every raw event (see [`BaganCache::on_change`]).
    ///
    /// The returned [`io::Result`] covers watcher setup errors only; runtime
    /// errors are printed to stderr by the watch callback.
    pub fn start_watching(&mut self) -> io::Result<()> {
        // The watcher thread shares the memory tier and archive handle
        // through clone-shared Arcs.
        let memory = self.memory.clone();
        let archive = self.archive.clone();
        let entry_dir = self.entry_dir.clone();
        let assets_dir = self.assets_dir.clone();
        let theme_assets_dir = self.theme_assets_dir.clone();
        let markdown_layouts = self.markdown_layouts.clone();
        let change_callbacks = self.change_callbacks.clone();
        // Liquid collection state (already flattened from directories):
        // new/removed `.html`/`.liquid` files inside the configured
        // collections update this shared list, so `liquids()` stays in
        // sync with the filesystem while watching. Directory-level
        // events re-flatten from the configured roots.
        let liquid_files = Arc::clone(&self.liquid_files);
        let liquid_roots = Arc::clone(&self.liquid_collection_roots);
        // Pretty logs render paths relative to the project root (the
        // cwd the cache was constructed in).
        let log_root = env::current_dir()?;
        // One log line per file operation: remember when each path last
        // logged so duplicate events from a single save (truncate +
        // write, atomic rename, …) coalesce into one line.
        let mut last_log_at: HashMap<PathBuf, Instant> = HashMap::new();

        let mut watcher: RecommendedWatcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                match res {
                    Ok(event) => {
                        for path in event.paths {
                            let is_markdown = path.starts_with(&entry_dir);
                            // The notify event kind cannot be trusted for
                            // the correct label (editors and `mv` surface
                            // atomic saves as rename/"modify" events), so
                            // classify from the actual filesystem state.
                            let kind_str = classify_change(&memory, &archive, &path);
                            change_callbacks.dispatch(CacheChangeEvent {
                                path: path.to_string_lossy().to_string(),
                                kind: kind_str.to_string(),
                                is_markdown,
                            });
                            // One log line per file operation, and only
                            // for files the cache could ever track: the
                            // same path may fire several events (rename
                            // pairs, atomic saves) and editor side files
                            // (`.tmp`, swap) never log.
                            let now = Instant::now();
                            if should_log_change(&path, &last_log_at, now) {
                                log_change_pretty(kind_str, &path, &log_root);
                                last_log_at.insert(path.clone(), now);
                            }
                            // Directory events and removals rescan everything.
                            // A removed directory no longer satisfies `is_dir`,
                            // and removed files must also be purged from both
                            // cache tiers instead of leaving stale entries.
                            if path.is_dir() || !path.exists() {
                                let mut new_map: HashMap<PathBuf, CachedContent> = HashMap::new();
                                match collect_markdown_content(&entry_dir, &markdown_layouts) {
                                    Ok(files) => {
                                        new_map.extend(
                                            files.into_iter().map(|(path, text)| {
                                                (path, CachedContent::Text(text))
                                            }),
                                        );
                                    }
                                    Err(error) => {
                                        eprintln!("watch error collecting markdown: {error}");
                                        continue;
                                    }
                                }
                                if let Some(ref a_dir) = assets_dir {
                                    match collect_asset_content(a_dir) {
                                        Ok(assets) => new_map.extend(assets),
                                        Err(error) => {
                                            eprintln!("watch error collecting assets: {error}");
                                            continue;
                                        }
                                    }
                                }
                                match collect_asset_content(&theme_assets_dir) {
                                    Ok(assets) => new_map.extend(assets),
                                    Err(error) => {
                                        eprintln!("watch error collecting theme assets: {error}");
                                        continue;
                                    }
                                }
                                // Re-resolve the liquid collections from
                                // their configured roots: the removed/added
                                // path may be a whole directory.
                                let fresh_liquids = {
                                    let roots = liquid_roots.lock().unwrap().clone();
                                    let mut fresh: HashMap<PathBuf, PathBuf> = HashMap::new();
                                    for root in &roots {
                                        if root.is_dir() {
                                            for path in file::collect_liquid_files(root) {
                                                fresh.insert(path, root.clone());
                                            }
                                        } else if root.is_file() && has_liquid_extension(root) {
                                            fresh.insert(root.clone(), entry_dir.clone());
                                        }
                                    }
                                    fresh
                                };
                                for path in fresh_liquids.keys() {
                                    if !new_map.contains_key(path)
                                        && let Ok(text) = fs::read_to_string(path)
                                    {
                                        new_map.insert(path.clone(), CachedContent::Text(text));
                                    }
                                }
                                *liquid_files.lock().unwrap() = fresh_liquids;
                                // Entries no longer part of any collection
                                // fall out of the replaced maps above.
                                memory.replace_all(new_map.clone());
                                archive.sync_all(new_map);
                                continue;
                            }

                            if path.is_file()
                                && let Some(ext) = path.extension().and_then(|e| e.to_str())
                            {
                                let ext_l = ext.to_lowercase();
                                if MARKDOWN_EXTENSIONS.contains(&ext_l.as_str())
                                    && path.starts_with(&entry_dir)
                                {
                                    match markdown_file_for_layouts(&path, &markdown_layouts) {
                                        Ok(Some(text)) => {
                                            let content = CachedContent::Text(text);
                                            memory.insert(path.clone(), content.clone());
                                            archive.upsert(path, content);
                                        }
                                        Ok(None) => {
                                            memory.remove(&path);
                                            archive.remove(path);
                                        }
                                        Err(error) => {
                                            eprintln!("watch error reading markdown: {error}");
                                            memory.remove(&path);
                                            archive.remove(path);
                                        }
                                    }
                                } else if ASSET_EXTENSIONS.contains(&ext_l.as_str())
                                    && (assets_dir
                                        .as_ref()
                                        .is_some_and(|dir| path.starts_with(dir))
                                        || path.starts_with(&theme_assets_dir))
                                {
                                    let is_binary =
                                        ASSET_BINARY_EXTENSIONS.contains(&ext_l.as_str());
                                    let content = if is_binary {
                                        fs::read(&path).ok().map(CachedContent::Binary)
                                    } else {
                                        fs::read_to_string(&path).ok().map(CachedContent::Text)
                                    };
                                    match content {
                                        Some(content) => {
                                            memory.insert(path.clone(), content.clone());
                                            archive.upsert(path, content);
                                        }
                                        None => {
                                            memory.remove(&path);
                                            archive.remove(path);
                                        }
                                    }
                                } else if has_liquid_extension(&path)
                                    && liquid_roots.lock().unwrap().iter().any(|root| {
                                        root.is_file() && root == &path
                                            || root.is_dir() && path.starts_with(root)
                                    })
                                {
                                    // A `.html`/`.liquid` file inside one of
                                    // the configured liquid collections:
                                    // keep its text current in both tiers
                                    // (new files join the tracked list,
                                    // removed ones drop out).
                                    match fs::read_to_string(&path) {
                                        Ok(text) => {
                                            let content = CachedContent::Text(text);
                                            {
                                                let mut tracked = liquid_files.lock().unwrap();
                                                let base = liquid_roots
                                                    .lock()
                                                    .unwrap()
                                                    .iter()
                                                    .find(|root| {
                                                        root.is_dir() && path.starts_with(root)
                                                    })
                                                    .cloned()
                                                    .unwrap_or_else(|| entry_dir.clone());
                                                tracked.entry(path.clone()).or_insert(base);
                                            }
                                            memory.insert(path.clone(), content.clone());
                                            archive.upsert(path, content);
                                        }
                                        Err(_) => {
                                            liquid_files.lock().unwrap().remove(&path);
                                            memory.remove(&path);
                                            archive.remove(path);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("watch error: {e:?}"),
                }
            })
            .map_err(|e| io::Error::other(format!("notify::recommended_watcher error: {e}")))?;

        watcher
            .watch(&self.entry_dir, RecursiveMode::Recursive)
            .map_err(|e| io::Error::other(format!("notify::watch error: {e}")))?;

        if let Some(a) = &self.assets_dir {
            watcher
                .watch(a, RecursiveMode::Recursive)
                .map_err(|e| io::Error::other(format!("notify::watch error: {e}")))?;
        }

        if self.theme_assets_dir.is_dir() {
            watcher
                .watch(&self.theme_assets_dir, RecursiveMode::Recursive)
                .map_err(|e| io::Error::other(format!("notify::watch error: {e}")))?;
        }

        self.watcher = Some(watcher);
        Ok(())
    }

    /// Stop watching the entry/assets directories and flush pending
    /// archive writes. The Rust counterpart of the JS `dispose()`: use it
    /// to shut the cache down gracefully — e.g. on `Ctrl+C` — instead of
    /// relying on the automatic [`Drop`] cleanup. Safe to call more than
    /// once (and without ever calling `start_watching`).
    pub fn stop_watching(&mut self) {
        self.watcher = None;
        // Releasing the keep-alive lets a Node process that only watches
        // exit again (the strong TSFN was the last ref'd loop handle).
        #[cfg(feature = "npm")]
        {
            self.watch_keep_alive = None;
        }
        self.archive.flush();
    }

    /// Read one entry: memory tier first, then the disk archive. Returns
    /// `None` when the path is unknown to both tiers.
    pub fn get(&self, path: &Path) -> Option<CachedContent> {
        if let Some(content) = self.memory.get(path) {
            return Some(content);
        }
        self.archive.read(path)
    }

    /// Clone of every text entry in memory: absolute path → raw text.
    ///
    /// This returns only entries currently in memory; use [`Self::get`] to
    /// read an individual entry from either the memory or archive tier.
    pub fn all(&self) -> HashMap<PathBuf, String> {
        self.memory
            .iter_all()
            .into_iter()
            .filter_map(|(p, c)| match c {
                CachedContent::Text(s) => Some((p, s)),
                _ => None,
            })
            .collect()
    }

    /// Union of the memory-tier and archive-tier keys.
    fn all_keys(&self) -> HashSet<PathBuf> {
        let mut keys: HashSet<PathBuf> = HashSet::new();
        for k in self.memory.keys() {
            keys.insert(k);
        }
        for k in self.archive.keys() {
            keys.insert(k);
        }
        keys
    }

    /// JS: `cache.getText(absPath)` — cached text of a file, or `null`.
    /// Reads fall through to the archive tier for evicted entries.
    #[cfg_attr(feature = "npm", napi)]
    pub fn get_text(&self, abs_path: String) -> Option<String> {
        self.get(Path::new(&abs_path)).and_then(|c| match c {
            CachedContent::Text(s) => Some(s),
            _ => None,
        })
    }

    /// JS: `cache.getBinary(absPath)` — cached bytes of a binary asset, or
    /// `null`. Reads fall through to the archive tier.
    #[cfg_attr(feature = "npm", napi)]
    pub fn get_binary(&self, abs_path: String) -> Option<Vec<u8>> {
        self.get(Path::new(&abs_path)).and_then(|c| match c {
            CachedContent::Binary(b) => Some(b),
            _ => None,
        })
    }

    /// JS: `cache.listFiles()` — absolute paths of everything cached
    /// (memory + archive tiers).
    #[cfg_attr(feature = "npm", napi)]
    pub fn list_files(&self) -> Vec<String> {
        self.all_keys()
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    /// JS: `cache.markdownFiles()` — absolute paths of cached markdown
    /// files (either tier).
    #[cfg_attr(feature = "npm", napi)]
    pub fn markdown_files(&self) -> Vec<String> {
        self.filter_cached_files(|ext| MARKDOWN_EXTENSIONS.contains(&ext))
    }

    /// Accepted Markdown layouts discovered from the theme's `.html`
    /// templates, sorted by name.
    ///
    /// JS: `cache.markdownLayouts()`.
    #[cfg_attr(feature = "npm", napi)]
    pub fn markdown_layouts(&self) -> Vec<String> {
        let mut layouts: Vec<String> = self.markdown_layouts.iter().cloned().collect();
        layouts.sort();
        layouts
    }

    /// Absolute paths to the configured theme's layouts, assets, and data
    /// directories.
    #[cfg_attr(feature = "npm", napi)]
    pub fn theme_dirs(&self) -> ThemeDirs {
        let dirs = ThemeDirs {
            assets_dir: self.theme_assets_dir.to_string_lossy().to_string(),
            layouts_dir: self.theme_layouts_dir.to_string_lossy().to_string(),
            data_dir: self.theme_data_dir.to_string_lossy().to_string(),
            includes_dir: self.theme_includes_dir.to_string_lossy().to_string(),
        };
        dirs
    }
    /// JS: `cache.assetFiles()` — absolute paths of cached non-markdown,
    /// non-liquid files (either tier).
    #[cfg_attr(feature = "npm", napi)]
    pub fn asset_files(&self) -> Vec<String> {
        self.filter_cached_files(|ext| {
            !MARKDOWN_EXTENSIONS.contains(&ext) && !LIQUID_EXTENSIONS.contains(&ext)
        })
    }

    /// Filter the union of memory + archive keys by extension predicate.
    fn filter_cached_files(&self, keep: fn(&str) -> bool) -> Vec<String> {
        self.all_keys()
            .into_iter()
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(keep)
                    .unwrap_or(false)
            })
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }
    // ===== Derived page / asset objects (Liquid template shapes) =====

    /// Rust: every page object — markdown pages plus the Liquid
    /// collection pages — sorted by slug. Both share the
    /// [`PageObject`] shape, so Liquid themes render them through the
    /// same template code.
    pub fn page_objects(&self) -> Vec<PageObject> {
        let mut pages: Vec<PageObject> = Vec::new();
        for path in self.all_keys() {
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| MARKDOWN_EXTENSIONS.contains(&e))
                .unwrap_or(false)
                && let Some(page) = self.page_object_for(&path)
            {
                pages.push(page);
            }
        }
        pages.extend(self.liquid_objects());
        pages.sort_by(|a, b| a.slug.cmp(&b.slug));
        pages
    }

    /// JS: `cache.pages()` — every page object, for Liquid themes
    /// (`{{ page.title }}`, `{{ page.layout }}`, …): markdown pages
    /// plus the Liquid collection pages.
    #[cfg_attr(feature = "npm", napi)]
    pub fn pages(&self) -> Vec<PageObject> {
        self.page_objects()
    }

    /// Build one [`PageObject`] for a markdown path (memory or archive
    /// tier). Returns `None` when the entry is unknown or unreadable.
    pub fn page_object_for(&self, path: &Path) -> Option<PageObject> {
        let content = self.get(path)?;
        let raw_md = match content {
            CachedContent::Text(s) => s,
            _ => return None,
        };
        let (md_body, fm) = match frontmatter(raw_md.to_string()) {
            Ok(parsed) => parsed.into_parts(),
            Err(_) => (raw_md.clone(), None),
        };
        let fm_ref = fm.as_ref();
        let layout = fm_string(fm_ref, &["layout"]).unwrap_or_else(|| "default".to_string());
        let title = extract_title(fm_ref, &md_body);
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let file_name = file_name_no_ext(&fname);
        let base_url = self.hosting.base_url();
        Some(PageObject {
            layout,
            file_name: file_name.clone(),
            slug: markdown_slug(path, &self.entry_dir, &base_url),
            raw: raw_md.clone(),
            html: render_markdown(
                &raw_md,
                &self.ast_opts,
                &self.render_opts,
                &self.plugins,
                Some(file_name),
            ),
            out_file_path: page_out_file(path, &self.entry_dir, &self.out_dir),
            input_file_path: path.to_string_lossy().to_string(),
            title,
            summary: fm_string(fm_ref, &["summary", "description"]),
            tags: fm_string_list(fm_ref, "tags"),
            publish_date: fm_string(fm_ref, &["date", "publishDate", "publish_date"]),
            extra: fm_ref.and_then(extra_frontmatter_fields),
        })
    }

    /// JS: `cache.getPage(inputPath)` — a single page object by absolute
    /// input path (markdown or Liquid), or `null`.
    #[cfg_attr(feature = "npm", napi)]
    pub fn get_page(&self, input_path: String) -> Option<PageObject> {
        let path = Path::new(&input_path);
        if has_liquid_extension(path) {
            self.liquid_object_for(path)
        } else {
            self.page_object_for(path)
        }
    }

    /// Rust: every asset object, sorted by slug. Liquid template files
    /// (the configured collections) are not assets — see
    /// [`Self::liquid_objects`].
    pub fn asset_objects(&self) -> Vec<AssetsObject> {
        let tracked_liquids = self.liquid_files.lock().unwrap().clone();
        let mut assets: Vec<AssetsObject> = Vec::new();
        for path in self.all_keys() {
            if path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| !MARKDOWN_EXTENSIONS.contains(&e))
                .unwrap_or(false)
                && !tracked_liquids.contains_key(&path)
                && let Some(asset) = self.asset_object_for(&path)
            {
                assets.push(asset);
            }
        }
        assets.sort_by(|a, b| a.slug.cmp(&b.slug));
        assets
    }

    /// JS: `cache.assets()` — every asset object.
    #[cfg_attr(feature = "npm", napi)]
    pub fn assets(&self) -> Vec<AssetsObject> {
        self.asset_objects()
    }

    /// Build one [`AssetsObject`] for an asset path (either tier).
    pub fn asset_object_for(&self, path: &Path) -> Option<AssetsObject> {
        let content = self.get(path)?;
        let base_dir = if path.starts_with(&self.theme_assets_dir) {
            &self.theme_assets_dir
        } else {
            self.assets_dir.as_deref().unwrap_or(&self.entry_dir)
        };
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .unwrap_or_default();
        let raw = match &content {
            CachedContent::Text(s) => s.clone(),
            CachedContent::Binary(b) => binary_to_base64(b),
        };
        let base_url = self.hosting.base_url();
        Some(AssetsObject {
            file_name: file_name_no_ext(&fname),
            slug: asset_slug(path, base_dir, &base_url),
            raw,
            out_file_path: asset_out_file(path, base_dir, &self.out_dir),
            input_file_path: path.to_string_lossy().to_string(),
            ext: ext.clone(),
            file_type: asset_file_type(&ext).to_string(),
        })
    }

    /// JS: `cache.getAsset(inputPath)` — a single asset object by absolute
    /// input path, or `null`.
    #[cfg_attr(feature = "npm", napi)]
    pub fn get_asset(&self, input_path: String) -> Option<AssetsObject> {
        self.asset_object_for(Path::new(&input_path))
    }

    // ===== Liquid collection objects (rendered on the JS side) =====

    /// Rust: every liquid page object, sorted by slug. Liquid pages are
    /// [`PageObject`] values just like markdown pages — same shape, so
    /// Liquid themes render them through the same template code.
    pub fn liquid_objects(&self) -> Vec<PageObject> {
        let tracked = self.liquid_files.lock().unwrap().clone();
        let mut liquids: Vec<PageObject> = tracked
            .iter()
            .filter_map(|(path, base)| self.liquid_object_for_with_base(path, base))
            .collect();
        liquids.sort_by(|a, b| a.slug.cmp(&b.slug));
        liquids
    }

    /// JS: `cache.liquids()` — every liquid page object for the
    /// configured `liquidCollections`: front-matter fields from the
    /// JS-side `liquidRender(file)` callback (which also renders the
    /// HTML), path-derived fields from the cache.
    #[cfg_attr(feature = "npm", napi)]
    pub fn liquids(&self) -> Vec<PageObject> {
        self.liquid_objects()
    }

    /// Build one [`PageObject`] for a liquid template path (either
    /// tier), slugging relative to its collection base directory: files
    /// from a directory collection slug relative to the collection root,
    /// single-file collections relative to the entry directory.
    pub fn liquid_object_for(&self, path: &Path) -> Option<PageObject> {
        let base = self
            .liquid_files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .unwrap_or_else(|| self.entry_dir.clone());
        self.liquid_object_for_with_base(path, &base)
    }

    /// [`Self::liquid_object_for`] with an explicit slug base directory.
    fn liquid_object_for_with_base(&self, path: &Path, base: &Path) -> Option<PageObject> {
        let content = self.get(path)?;
        let raw = match content {
            CachedContent::Text(s) => s,
            _ => return None,
        };
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // Front-matter fallback: the raw liquid files carry YAML front
        // matter like markdown pages, so fields the renderer does not
        // provide still surface.
        let fm = frontmatter(raw.clone())
            .ok()
            .and_then(|parsed| parsed.yaml_data().cloned());
        let fm_ref = fm.as_ref();
        let fallback_layout = fm_string(fm_ref, &["layout"]);
        let fallback_title = fm_string(fm_ref, &["title"]);
        let fallback_summary = fm_string(fm_ref, &["summary", "description"]);
        let fallback_tags = fm_string_list(fm_ref, "tags");
        let fallback_publish_date = fm_string(fm_ref, &["date", "publishDate", "publish_date"]);
        let fallback_extra = fm_ref.and_then(extra_frontmatter_fields);
        // The renderer (JS `liquidRender` callback or Rust closure)
        // extracts the front matter, renders the Liquid content to HTML,
        // and returns the page fields. Fields the renderer does not
        // provide fall back to the raw front matter the cache parsed.
        let rendered = self.render_liquid(&raw);
        let layout = rendered
            .as_ref()
            .filter(|r| !r.layout.is_empty())
            .map(|r| r.layout.clone())
            .or_else(|| fallback_layout.clone())
            .unwrap_or_else(|| "default".to_string());
        let title = rendered
            .as_ref()
            .and_then(|r| r.title.clone())
            .or(fallback_title);
        let summary = rendered
            .as_ref()
            .and_then(|r| r.summary.clone())
            .or(fallback_summary);
        let tags = rendered
            .as_ref()
            .and_then(|r| r.tags.clone())
            .or(fallback_tags);
        let publish_date = rendered
            .as_ref()
            .and_then(|r| r.publish_date.clone())
            .or(fallback_publish_date);
        let extra = rendered
            .as_ref()
            .and_then(|r| r.extra.clone())
            .or(fallback_extra);
        let html = rendered.map(|r| r.html).unwrap_or_default();
        let base_url = self.hosting.base_url();
        Some(PageObject {
            layout,
            file_name: liquid_file_name_no_ext(&fname),
            slug: liquid_slug(path, base, &base_url),
            raw,
            html,
            out_file_path: liquid_out_file(path, base, &self.out_dir),
            input_file_path: path.to_string_lossy().to_string(),
            title,
            summary,
            tags,
            publish_date,
            extra,
        })
    }

    /// JS: `cache.getLiquid(inputPath)` — a single liquid page object by
    /// absolute input path, or `null`.
    #[cfg_attr(feature = "npm", napi)]
    pub fn get_liquid(&self, input_path: String) -> Option<PageObject> {
        self.liquid_object_for(Path::new(&input_path))
    }

    /// JS: `cache.liquidFiles()` — absolute paths of the tracked liquid
    /// template files (either tier).
    #[cfg_attr(feature = "npm", napi)]
    pub fn liquid_files(&self) -> Vec<String> {
        self.liquid_files
            .lock()
            .unwrap()
            .keys()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    /// Render a liquid collection file through the registered renderer.
    /// Returns `None` when no renderer is configured or the renderer
    /// fails; renderer errors print to stderr (the caller falls back to
    /// the raw front matter parsed by the cache).
    fn render_liquid(&self, raw: &str) -> Option<PageObject> {
        let renderer = &self.liquid_renderer;
        #[cfg(feature = "npm")]
        {
            let renderer = renderer.as_ref()?;
            let env = self.liquid_env.as_ref()?;
            let Ok(func) = renderer.borrow_back(env) else {
                eprintln!("liquidRender callback could not be invoked");
                return None;
            };
            // A single-argument call: the raw file text (front matter +
            // Liquid content) goes straight to the JS callback. The
            // result is converted leniently — JS consumers may return a
            // partial object; every field falls back to the raw front
            // matter when the callback does not provide it.
            let value: serde_json::Value = match func.call(raw.to_string()) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("liquidRender callback failed: {error}");
                    return None;
                }
            };
            let value_ref = value.as_object();
            let string_field = |keys: &[&str]| -> Option<String> {
                value_ref.and_then(|obj| {
                    keys.iter()
                        .find_map(|key| obj.get(*key))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
            };
            let string_list = |key: &str| -> Option<Vec<String>> {
                value_ref
                    .and_then(|obj| obj.get(key))
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .map(|v| match v {
                                serde_json::Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .collect()
                    })
            };
            Some(PageObject {
                // `layout` stays empty when the callback does not
                // provide it — the merge falls back to the raw front
                // matter, and only then to "default".
                layout: string_field(&["layout"]).unwrap_or_default(),
                title: string_field(&["title"]),
                summary: string_field(&["summary", "description"]),
                tags: string_list("tags"),
                publish_date: string_field(&["publishDate", "publish_date", "date"]),
                extra: value_ref.and_then(|obj| {
                    let named = [
                        "layout",
                        "title",
                        "summary",
                        "tags",
                        "date",
                        "publish_date",
                        "publishDate",
                        "html",
                        "fileName",
                        "slug",
                        "raw",
                        "outFilePath",
                        "inputFilePath",
                    ];
                    let mut extra: HashMap<String, String> = HashMap::new();
                    for (k, v) in obj {
                        if named.contains(&k.as_str()) {
                            continue;
                        }
                        let rendered = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        extra.insert(k.clone(), rendered);
                    }
                    (!extra.is_empty()).then_some(extra)
                }),
                html: string_field(&["html"]).unwrap_or_default(),
                file_name: String::new(),
                slug: String::new(),
                raw: String::new(),
                out_file_path: String::new(),
                input_file_path: String::new(),
            })
        }
        #[cfg(not(feature = "npm"))]
        {
            renderer.as_ref().map(|render| render(raw.to_string()))
        }
    }

    /// Parsed theme metadata exposed as an object to Rust and JavaScript.
    #[cfg_attr(feature = "npm", napi)]
    pub fn theme_config(&self) -> BaganThemeConfig {
        self.theme_config.clone()
    }

    // ===== Archive-tier introspection =====

    /// JS: `cache.archived()` — absolute paths flushed to the disk
    /// archive. Everything not listed still lives in memory only.
    #[cfg_attr(feature = "npm", napi)]
    pub fn archived(&self) -> Vec<String> {
        self.archive
            .keys()
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    /// JS: `cache.mapJson()` — the current `map.json` document (path →
    /// shard location + metadata, including front matter for search).
    #[cfg_attr(feature = "npm", napi)]
    pub fn map_json(&self) -> String {
        self.archive.map_json()
    }

    /// JS: `cache.flush()` — write all pending archive entries now,
    /// blocking until the worker is done.
    #[cfg_attr(feature = "npm", napi)]
    pub fn flush(&self) {
        self.archive.flush();
    }

    // ===== Hosting / baseUrl =====

    /// JS: `cache.baseUrl()` — the resolved base URL path prefix for the
    /// configured hosting (e.g. `/repo/` for a GitHub Pages project site).
    #[cfg_attr(feature = "npm", napi)]
    pub fn base_url(&self) -> String {
        self.hosting.base_url()
    }

    /// JS: `cache.resolveUrl(path)` — prefix a site-relative path with the
    /// resolved base URL: `resolveUrl("docs/intro")` → `/repo/docs/intro`.
    #[cfg_attr(feature = "npm", napi)]
    pub fn resolve_url(&self, path: String) -> String {
        let base = self.hosting.base_url();
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            base
        } else {
            format!("{}/{}", base.trim_end_matches('/'), path)
        }
    }

    /// JS: `cache.siteUrl()` — absolute origin URL for canonical URLs, or
    /// `null` when not configured.
    #[cfg_attr(feature = "npm", napi)]
    pub fn site_url(&self) -> Option<String> {
        self.hosting.site_url()
    }

    // ===== Subscriptions and disposal =====

    /// JS: `cache.onChange(cb)` — invoke `cb(event)` on every file change the
    /// watcher detects while watching is active. Returns a subscription id
    /// that can be passed to `offChange(id)`.
    ///
    /// The callback is wrapped in a `ThreadsafeFunction` because notify
    /// events fire on the watcher thread, not the JS main thread.
    #[cfg(feature = "npm")]
    #[napi]
    pub fn on_change(
        &self,
        #[napi(ts_arg_type = "(event: CacheChangeEvent) => void")] callback: Function<
            'static,
            CacheChangeEvent,
            Unknown<'static>,
        >,
    ) -> Result<u32> {
        use napi::threadsafe_function::ThreadsafeFunctionCallMode;

        // Weak TSFN: the subscription must not keep the Node event loop
        // alive on its own — long-running consumers (dev server) keep the
        // loop alive via their own handles (http listener), so an
        // onChange-only script should exit when its work is done.
        let tsfn = callback
            .build_threadsafe_function::<CacheChangeEvent>()
            .weak::<true>()
            .build()?;

        Ok(self
            .change_callbacks
            .add(Box::new(move |event: CacheChangeEvent| {
                tsfn.call(event, ThreadsafeFunctionCallMode::NonBlocking);
            })))
    }

    /// Register a Rust callback that receives every change detected while
    /// watching is active. Returns a subscription id that can be passed to
    /// [`BaganCache::off_change`].
    #[cfg(not(feature = "npm"))]
    pub fn on_change<F>(&self, callback: F) -> u32
    where
        F: Fn(CacheChangeEvent) + Send + 'static,
    {
        self.change_callbacks.add(Box::new(callback))
    }

    /// JS: `cache.offChange(id)` — remove a previously registered callback.
    #[cfg_attr(feature = "npm", napi)]
    pub fn off_change(&self, id: u32) {
        self.change_callbacks.remove(id);
    }

    /// JS: `cache.dispose()` — stop the watcher, flush pending archive
    /// writes, and drop JS change callbacks. Safe to call more than once.
    ///
    /// Rust consumers do not need this: dropping the [`BaganCache`] stops
    /// the watcher, flushes the archive, and joins the workers.
    #[cfg_attr(feature = "npm", napi)]
    pub fn dispose(&mut self) {
        self.stop_watching();
        self.change_callbacks.clear();
    }
}

/// JS-side options accepted by the `BaganCache` constructor (a flattened
/// subset of [`BaganCacheConfig`]; memory budgets only apply through the
/// `withConfig` factory).
#[cfg(feature = "npm")]
#[napi(object)]
pub struct BaganCacheJsOptions {
    /// Theme configuration object (`themeConfig`) — imported directly on
    /// the JS side (e.g. `import theme from "my-bagan-theme"` and pass
    /// its default export). When omitted, the theme directories below
    /// are resolved from the entry directory instead.
    pub theme_config: Option<BaganThemeConfig>,
    /// Theme layouts directory (used when `themeConfig` is omitted),
    /// relative to the entry directory. Defaults to `_layouts`.
    pub theme_layouts_dir: Option<String>,
    /// Theme includes directory (used when `themeConfig` is omitted),
    /// relative to the entry directory. Defaults to `_includes`.
    pub theme_includes_dir: Option<String>,
    /// Theme assets directory (used when `themeConfig` is omitted),
    /// relative to the entry directory. Defaults to `_assets`.
    pub theme_assets_dir: Option<String>,
    /// Theme data directory (used when `themeConfig` is omitted),
    /// relative to the entry directory. Defaults to `_data`.
    pub theme_data_dir: Option<String>,
    /// Markdown parsing/rendering options (`fragment`, GFM, …).
    pub markdown: Option<PeisarOptions>,
    /// Disk-archive thresholds (shard size / flush interval).
    pub archive: Option<ArchiveConfig>,
    /// Static-hosting baseUrl resolution.
    pub hosting: Option<HostingConfig>,
    /// Build output directory.
    pub out_dir: Option<String>,
    /// Disk-archive directory.
    pub cache_dir: Option<String>,
    /// Liquid (`.html`) collections inside the entry directory: each
    /// entry is a directory (flattened recursively) or a single file,
    /// resolved against the entry directory. The discovered files are
    /// cached as text, watched, and exposed through `cache.liquids()`.
    pub liquid_collections: Option<Vec<String>>,
    /// JavaScript function that renders a Liquid collection file to a
    /// page (`liquidRender(file: string) => PageObject`). The
    /// callback extracts the YAML front matter from the raw file,
    /// renders the Liquid content to HTML, and returns the page fields
    /// (the object is converted leniently — `html` is the meaningful
    /// field, anything else falls back to the raw front matter); the
    /// cache fills the path-derived fields (fileName, slug,
    /// outFilePath, inputFilePath) and `raw`. Invoked on the JS main
    /// thread when liquid objects are built; a callback that throws or
    /// returns nothing leaves the front-matter fields empty and `html`
    /// as the raw template. Registerable later with `useLiquidRender()`
    /// as well.
    #[napi(ts_type = "(file: string) => PageObject")]
    pub liquid_render: Option<Function<'static, String, serde_json::Value>>,
}

// Dropping a `BaganCache` is the Rust equivalent of the JS `dispose()`:
// the watcher is released with the struct, the archive worker flushes its
// pending buffer and exits, and the memory tier goes away with it. In npm
// builds the event-loop keep-alive goes with it too, so a Node process
// holding only the cache can exit once the cache is garbage-collected.
impl Drop for BaganCache {
    fn drop(&mut self) {
        self.watcher = None;
        #[cfg(feature = "npm")]
        {
            self.watch_keep_alive = None;
        }
        self.change_callbacks.clear();
        self.archive.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    static TEST_CWD_LOCK: Mutex<()> = Mutex::new(());

    fn make_tmp(tag: &str) -> io::Result<(PathBuf, PathBuf)> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| io::Error::other(format!("time error: {e}")))?
            .as_nanos();
        let tmp = env::temp_dir().join(format!("peisar_test_{tag}_{now}"));
        if tmp.exists() {
            fs::remove_dir_all(&tmp)?;
        }
        fs::create_dir_all(&tmp)?;
        let local_theme = tmp.join("_layouts");
        fs::create_dir_all(&local_theme)?;
        for layout in ["default", "docs", "blog", "page", "home"] {
            fs::write(local_theme.join(format!("{layout}.html")), "")?;
        }
        fs::write(local_theme.join("ignored.txt"), "")?;
        let orig = env::current_dir()?;
        Ok((tmp, orig))
    }

    fn write_md(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = File::create(p).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.sync_all().unwrap();
    }

    /// Write one (empty) layout template into `dir/_layouts`.
    fn write_layout(dir: &Path, name: &str) {
        let layouts = dir.join("_layouts");
        fs::create_dir_all(&layouts).unwrap();
        fs::write(layouts.join(format!("{name}.html")), "").unwrap();
    }

    /// Construct a cache over a relative entry dir from the tests —
    /// the Rust `BaganCache::new` is cfg-gated out of npm builds (the
    /// napi constructor takes an `Env`), so the tests use
    /// `with_config` directly.
    fn cache_for(entry_dir: &str) -> io::Result<BaganCache> {
        BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some(entry_dir.to_string()),
            ..Default::default()
        })
    }

    fn restore(orig: PathBuf, tmp: PathBuf) {
        let _ = env::set_current_dir(orig);
        let _ = fs::remove_dir_all(tmp);
    }

    #[test]
    fn default_entry_and_layout_filter() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("layoutfilter")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "index.md", "---\nlayout: default\n---\n# Home\n");
        write_md(&tmp, "docs/page.md", "---\nlayout: docs\n---\n# Docs\n");
        write_md(&tmp, "blog/post.md", "---\nlayout: blog\n---\n# Blog\n");
        write_md(&tmp, "custom.md", "---\nlayout: custom\n---\n# Custom\n");
        write_md(&tmp, "missing-layout.md", "# No layout\n");
        fs::write(tmp.join("_layouts").join("custom.html"), "")?;

        let cache = BaganCache::with_config(BaganCacheConfig {
            ..Default::default()
        })?;

        assert_eq!(
            cache.markdown_layouts(),
            ["blog", "custom", "default", "docs", "home", "page"]
        );
        assert_eq!(cache.markdown_files().len(), 4);
        assert!(
            cache
                .markdown_files()
                .iter()
                .any(|path| path.ends_with("custom.md"))
        );
        assert!(
            !cache
                .markdown_files()
                .iter()
                .any(|path| path.ends_with("missing-layout.md"))
        );

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn local_theme_dirs_default_to_entry_dir() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("localdirs")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: home\n---\n# Home\n",
        );
        write_layout(&tmp.join("contents"), "home");
        fs::create_dir_all(tmp.join("contents/_assets/css"))?;
        fs::write(tmp.join("contents/_assets/css/site.css"), "body {}")?;
        fs::create_dir_all(tmp.join("contents/_includes"))?;
        fs::create_dir_all(tmp.join("contents/_data"))?;

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            ..Default::default()
        })?;

        // No themeConfig: the theme dirs resolve from the entry dir.
        let dirs = cache.theme_dirs();
        assert_eq!(
            dirs.layouts_dir,
            tmp.join("contents/_layouts").to_string_lossy()
        );
        assert_eq!(
            dirs.includes_dir,
            tmp.join("contents/_includes").to_string_lossy()
        );
        assert_eq!(
            dirs.assets_dir,
            tmp.join("contents/_assets").to_string_lossy()
        );
        assert_eq!(dirs.data_dir, tmp.join("contents/_data").to_string_lossy());
        assert_eq!(cache.pages().len(), 1);
        let assets = cache.assets();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].slug, "/css/site.css");
        assert_eq!(assets[0].raw, "body {}");

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn local_theme_dirs_can_be_overridden_in_config() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("localdirsconfig")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: main\n---\n# Home\n",
        );
        fs::create_dir_all(tmp.join("contents/templates"))?;
        fs::write(tmp.join("contents/templates/main.html"), "")?;
        fs::create_dir_all(tmp.join("contents/partial"))?;
        fs::create_dir_all(tmp.join("contents/static/img"))?;
        fs::write(tmp.join("contents/static/img/logo.png"), vec![1u8, 2])?;
        fs::create_dir_all(tmp.join("contents/dataset"))?;

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            theme_layouts_dir: Some("templates".into()),
            theme_includes_dir: Some("partial".into()),
            theme_assets_dir: Some("static".into()),
            theme_data_dir: Some("dataset".into()),
            ..Default::default()
        })?;

        let dirs = cache.theme_dirs();
        assert_eq!(
            dirs.layouts_dir,
            tmp.join("contents/templates").to_string_lossy()
        );
        assert_eq!(
            dirs.includes_dir,
            tmp.join("contents/partial").to_string_lossy()
        );
        assert_eq!(
            dirs.assets_dir,
            tmp.join("contents/static").to_string_lossy()
        );
        assert_eq!(
            dirs.data_dir,
            tmp.join("contents/dataset").to_string_lossy()
        );
        assert!(
            cache
                .markdown_layouts()
                .iter()
                .any(|layout| layout == "main")
        );
        assert_eq!(cache.assets().len(), 1);

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn theme_config_and_assets_are_loaded() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("theme")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: home\n---\n# Home\n",
        );
        // The theme directories now live inside the entry directory; a
        // themeConfig customizes their names.
        fs::create_dir_all(tmp.join("contents/theme/css"))?;
        fs::create_dir_all(tmp.join("contents/theme/layouts"))?;
        fs::write(tmp.join("contents/theme/layouts/home.html"), "")?;
        fs::write(tmp.join("contents/theme/css/theme.css"), "body {}")?;

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            theme_config: Some(BaganThemeConfig {
                name: Some("custom-theme".into()),
                version: Some("2.0.0".into()),
                layouts_dir: Some("theme/layouts".into()),
                assets_dir: Some("theme".into()),
                ..Default::default()
            }),
            ..Default::default()
        })?;

        let config = cache.theme_config();
        assert_eq!(config.name.as_deref(), Some("custom-theme"));
        assert_eq!(config.version.as_deref(), Some("2.0.0"));
        assert_eq!(config.author, None);
        let dirs = cache.theme_dirs();
        assert_eq!(
            dirs.layouts_dir,
            tmp.join("contents/theme/layouts").to_string_lossy()
        );
        assert_eq!(
            dirs.assets_dir,
            tmp.join("contents/theme").to_string_lossy()
        );
        assert_eq!(cache.pages().len(), 1);
        let assets = cache.assets();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].slug, "/css/theme.css");
        assert_eq!(assets[0].raw, "body {}");

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn configured_layout_directory_drives_discovery() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("customlayoutdir")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "index.md", "---\nlayout: custom\n---\n# Custom\n");
        fs::create_dir_all(tmp.join("templates"))?;
        fs::write(tmp.join("templates/custom.html"), "")?;
        fs::write(tmp.join("templates/ignored.txt"), "")?;

        let cache = BaganCache::with_config(BaganCacheConfig {
            theme_config: Some(BaganThemeConfig {
                layouts_dir: Some("templates".into()),
                ..Default::default()
            }),
            ..Default::default()
        })?;

        assert!(
            cache
                .markdown_layouts()
                .iter()
                .any(|layout| layout == "custom")
        );
        assert_eq!(cache.markdown_files().len(), 1);
        assert_eq!(cache.page_objects().len(), 1);

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn theme_without_html_layouts_rejects_markdown_pages() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("emptylayouts")?;
        env::set_current_dir(&tmp)?;
        let theme_layouts_dir = tmp.join("_layouts");
        fs::remove_dir_all(&theme_layouts_dir)?;
        fs::create_dir_all(&theme_layouts_dir)?;
        write_md(&tmp, "index.md", "---\nlayout: default\n---\n# Home\n");

        let cache = BaganCache::with_config(BaganCacheConfig::default())?;

        assert!(cache.markdown_layouts().is_empty());
        assert!(cache.markdown_files().is_empty());
        assert!(cache.page_objects().is_empty());

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn missing_index_is_an_error() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("missingindex")?;
        env::set_current_dir(&tmp)?;
        fs::create_dir("contents")?;

        let error = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => return Err(io::Error::other("missing index unexpectedly succeeded")),
        };
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("index"));

        // A plain index.html without a front-matter layout is NOT an
        // entry index — the check looks for a `layout` value.
        fs::write("contents/index.html", "<p>no layout</p>")?;
        let error = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => return Err(io::Error::other("plain index.html unexpectedly succeeded")),
        };
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("index"));

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn liquid_index_html_satisfies_entry_requirement() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("liquidindex")?;
        env::set_current_dir(&tmp)?;
        // No markdown index at all: a Liquid index.html with a
        // front-matter layout is the entry page.
        fs::create_dir_all("contents/_layouts")?;
        fs::write("contents/_layouts/home.html", "")?;
        fs::write(
            "contents/index.html",
            "---\nlayout: home\n---\n<h1>Home</h1>\n",
        )?;

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            liquid_collections: Some(vec![".".into()]),
            ..Default::default()
        })?;

        // The liquid index is tracked as a page object.
        let index_page = cache
            .pages()
            .into_iter()
            .find(|p| p.input_file_path.ends_with("index.html"));
        assert!(index_page.is_some(), "liquid index.html is not a page");
        let index_page = index_page.unwrap();
        assert_eq!(index_page.slug, "/");
        assert_eq!(index_page.out_file_path, ".bagan/index.html");
        assert_eq!(index_page.layout, "home");

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn assets_must_exist_inside_entry_directory() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("assetvalidation")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: default\n---\n# Home\n",
        );
        fs::create_dir_all("contents/assets")?;
        fs::create_dir("outside")?;

        let missing = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            assets_dir: Some("contents/missing".into()),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => return Err(io::Error::other("missing assets unexpectedly succeeded")),
        };
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);

        let outside = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            assets_dir: Some("outside".into()),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => return Err(io::Error::other("outside assets unexpectedly succeeded")),
        };
        assert_eq!(outside.kind(), io::ErrorKind::InvalidInput);

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn invalid_front_matter_is_a_collection_error() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("invalidfrontmatter")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: default\n---\n# Home\n",
        );
        write_md(
            &tmp,
            "contents/broken.md",
            "---\nlayout: [\n---\n# Broken\n",
        );

        let error = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => {
                return Err(io::Error::other(
                    "invalid front matter unexpectedly succeeded",
                ));
            }
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        restore(orig, tmp);
        Ok(())
    }

    #[cfg(not(feature = "npm"))]
    #[test]
    fn on_change_registers_a_rust_callback() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("change")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "index.md", "---\nlayout: default\n---\n# Home\n");

        let cache = BaganCache::new(&tmp)?;
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_callback = Arc::clone(&received);
        let subscription = cache.on_change(move |event| {
            received_callback.lock().unwrap().push(event);
        });

        cache.change_callbacks.dispatch(CacheChangeEvent {
            path: "/tmp/example.md".to_string(),
            kind: "modify".to_string(),
            is_markdown: true,
        });
        cache.off_change(subscription);

        assert_eq!(received.lock().unwrap().len(), 1);
        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn initial_snapshot_flushes_without_waiting_for_interval() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("memfirst")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "docs/index.md", "---\nlayout: default\n---\n# Home\n");
        write_md(
            &tmp,
            "docs/doc1.md",
            "---\ntitle: Test\nlayout: default\n---\n# Hello\n",
        );
        write_layout(&tmp.join("docs"), "default");

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("docs".into()),
            archive: Some(ArchiveConfig {
                flush_interval_ms: Some(60_000),
                ..Default::default()
            }),
            ..Default::default()
        })?;

        // Memory tier has the content immediately.
        let all = cache.all();
        assert_eq!(all.len(), 2);
        let content = cache.get(&tmp.join("docs").join("doc1.md")).unwrap();
        assert!(matches!(content, CachedContent::Text(ref s) if s.contains("# Hello")));

        let archive_dir = tmp.join(DEFAULT_ARCHIVE_DIR_NAME);
        let deadline = Instant::now() + Duration::from_secs(1);
        while cache.archived().len() != 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(cache.archived().len(), 2);
        assert!(archive_dir.join("map.json").exists());

        drop(cache);
        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn flush_writes_map_and_shards() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("flush")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "docs/index.md", "---\nlayout: default\n---\n# Home\n");
        write_md(&tmp, "docs/a.md", "---\nlayout: default\n---\n# A\n");
        write_layout(&tmp.join("docs"), "default");

        let cache = cache_for("docs")?;
        cache.flush();

        let archive_dir = tmp.join(DEFAULT_ARCHIVE_DIR_NAME);
        let map_str = fs::read_to_string(archive_dir.join("map.json"))?;
        let map: serde_json::Value = serde_json::from_str(&map_str).unwrap();
        assert_eq!(map["version"], 1);
        assert_eq!(
            map["files"]["docs/a.md"]["frontmatter"]["layout"],
            "default"
        );

        assert_eq!(cache.archived().len(), 2);
        assert!(archive_dir.join("shard-000001.bin").exists());

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn page_objects_liquid_shape() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("pages")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\ntitle: Home Page\nlayout: custom\nsummary: hi there\ntags:\n  - a\n  - b\ndate: 2026-01-01\nauthor: someone\n---\n# Not the title\n\nBody\n",
        );
        write_md(
            &tmp,
            "contents/docs/intro.md",
            "---\nlayout: docs\n---\n# Intro\n",
        );
        write_md(&tmp, "outside.md", "---\nlayout: custom\n---\n# Outside\n");
        write_layout(&tmp.join("contents"), "custom");
        write_layout(&tmp.join("contents"), "docs");

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            markdown: Some(PeisarOptions {
                fragment: Some(true),
                ..Default::default()
            }),
            hosting: Some(HostingConfig {
                provider: Some(HostingProvider::GithubPages),
                repo: Some("my-repo".into()),
                ..Default::default()
            }),
            out_dir: Some("dist".into()),
            ..Default::default()
        })?;

        let pages = cache.page_objects();
        assert_eq!(pages.len(), 2);
        assert!(
            pages
                .iter()
                .all(|page| !page.input_file_path.ends_with("outside.md"))
        );

        let home = pages
            .iter()
            .find(|p| p.input_file_path.ends_with("index.md"))
            .unwrap();
        assert_eq!(home.layout, "custom");
        assert_eq!(home.title.as_deref(), Some("Home Page"));
        assert_eq!(home.summary.as_deref(), Some("hi there"));
        assert_eq!(home.tags, Some(vec!["a".to_string(), "b".to_string()]));
        assert_eq!(home.publish_date.as_deref(), Some("2026-01-01"));
        // extra fields are flattened and stringified
        assert_eq!(home.extra.as_ref().unwrap()["author"], "someone");
        // baseUrl applied
        assert_eq!(home.slug, "/my-repo/");
        assert_eq!(home.out_file_path, "dist/index.html");
        assert!(home.html.contains("Body"));

        let intro = pages
            .iter()
            .find(|p| p.input_file_path.ends_with("intro.md"))
            .unwrap();
        // Title falls back to the first heading
        assert_eq!(intro.title.as_deref(), Some("Intro"));
        assert_eq!(intro.slug, "/my-repo/docs/intro");
        assert_eq!(intro.out_file_path, "dist/docs/intro/index.html");
        assert_eq!(intro.layout, "docs");

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn asset_objects_shape() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("assets")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: default\n---\n# Home\n",
        );
        fs::create_dir_all(tmp.join("contents/public").join("img"))?;
        fs::write(
            tmp.join("contents/public").join("img").join("logo.png"),
            vec![1u8, 2, 3],
        )?;
        write_layout(&tmp.join("contents"), "default");

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            assets_dir: Some("contents/public".into()),
            ..Default::default()
        })?;

        let assets = cache.asset_objects();
        assert_eq!(assets.len(), 1);
        let logo = &assets[0];
        assert_eq!(logo.file_name, "logo");
        assert_eq!(logo.ext, "png");
        assert_eq!(logo.file_type, "Image");
        assert_eq!(logo.slug, "/img/logo.png");
        // DEFAULT_OUT_DIR is ".bagan" when outDir is not configured.
        assert_eq!(logo.out_file_path, ".bagan/img/logo.png");
        // binary raw is base64
        assert_eq!(logo.raw, binary_to_base64(&[1u8, 2, 3]));

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn liquid_collections_flatten_and_render() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("liquid")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: default\n---\n# Home\n",
        );
        write_layout(&tmp.join("contents"), "default");
        // Collection directory with nested liquid files + a single file.
        // Liquid files carry YAML front matter like markdown pages.
        fs::create_dir_all(tmp.join("contents/_pages/docs"))?;
        fs::write(
            tmp.join("contents/_pages/about.html"),
            "---\nlayout: page\ntitle: About Page\ntags:\n  - one\n  - two\ncustom: x\n---\n<h1>About {{ site }}</h1>\n",
        )?;
        fs::write(tmp.join("contents/_pages/docs/guide.html"), "<p>Guide</p>")?;
        fs::write(tmp.join("contents/contact.liquid"), "<p>Contact</p>")?;
        #[allow(unused_mut)]
        let mut cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            liquid_collections: Some(vec!["_pages".into(), "contact.liquid".into()]),
            ..Default::default()
        })?;
        // Rust-side renderer (JS consumers pass `liquidRender` instead):
        // extracts the front matter, renders the content, and returns
        // the front-matter fields.
        #[cfg(not(feature = "npm"))]
        cache.use_liquid_render(|file: String| {
            let parsed = frontmatter(file.clone())
                .ok()
                .and_then(|p| p.yaml_data().cloned());
            PageObject {
                layout: fm_string(parsed.as_ref(), &["layout"])
                    .unwrap_or_else(|| "default".to_string()),
                title: fm_string(parsed.as_ref(), &["title"]),
                summary: fm_string(parsed.as_ref(), &["summary", "description"]),
                tags: fm_string_list(parsed.as_ref(), "tags"),
                publish_date: fm_string(parsed.as_ref(), &["date", "publishDate", "publish_date"]),
                extra: parsed.as_ref().and_then(extra_frontmatter_fields),
                html: file.replace("{{ site }}", "Bagan"),
                file_name: String::new(),
                slug: String::new(),
                raw: String::new(),
                out_file_path: String::new(),
                input_file_path: String::new(),
            }
        });

        // Directories are flattened and merged with configured files.
        let files = cache.liquid_files();
        assert_eq!(files.len(), 3);
        let liquids = cache.liquid_objects();
        assert_eq!(liquids.len(), 3);

        // pages() merges the markdown pages with the liquid pages —
        // both are PageObject values.
        let pages = cache.page_objects();
        assert_eq!(pages.len(), 4);
        assert!(
            pages
                .iter()
                .any(|p| p.input_file_path.ends_with("about.html"))
        );
        assert!(
            pages
                .iter()
                .any(|p| p.input_file_path.ends_with("index.md"))
        );

        let about = liquids
            .iter()
            .find(|l| l.input_file_path.ends_with("about.html"))
            .unwrap();
        assert_eq!(about.file_name, "about");
        assert_eq!(about.slug, "/about");
        assert_eq!(about.out_file_path, ".bagan/about.html");
        assert!(about.raw.contains("layout: page"));
        // Front-matter fields share the PageObject shape.
        #[cfg(not(feature = "npm"))]
        {
            assert_eq!(about.layout, "page");
            assert_eq!(about.title.as_deref(), Some("About Page"));
            assert_eq!(about.tags, Some(vec!["one".to_string(), "two".to_string()]));
            assert_eq!(about.extra.as_ref().unwrap()["custom"], "x");
            assert_eq!(
                about.html,
                "---\nlayout: page\ntitle: About Page\ntags:\n  - one\n  - two\ncustom: x\n---\n<h1>About Bagan</h1>\n"
            );
        }
        // Without a renderer (npm test runs): the cache still parses the
        // raw front matter, but html stays empty.
        #[cfg(feature = "npm")]
        {
            assert_eq!(about.layout, "page");
            assert_eq!(about.title.as_deref(), Some("About Page"));
            assert!(about.html.is_empty());
        }

        let guide = liquids
            .iter()
            .find(|l| l.input_file_path.ends_with("guide.html"))
            .unwrap();
        assert_eq!(guide.slug, "/docs/guide");
        assert_eq!(guide.out_file_path, ".bagan/docs/guide.html");
        // No front matter → layout falls back to "default".
        assert_eq!(guide.layout, "default");

        let contact = liquids
            .iter()
            .find(|l| l.input_file_path.ends_with("contact.liquid"))
            .unwrap();
        assert_eq!(contact.slug, "/contact");
        assert_eq!(contact.out_file_path, ".bagan/contact.html");

        // Liquid files are cached (either tier) and stay out of
        // `assets()`/`assetFiles()`.
        assert_eq!(cache.asset_objects().len(), 0);
        assert!(
            !cache
                .asset_files()
                .iter()
                .any(|p| p.ends_with("about.html"))
        );
        // Without a renderer the front-matter fields come from the raw
        // front matter and html stays empty.
        let bare = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            liquid_collections: Some(vec!["_pages".into(), "contact.liquid".into()]),
            ..Default::default()
        })?;
        let bare_about = bare
            .liquid_objects()
            .into_iter()
            .find(|l| l.input_file_path.ends_with("about.html"))
            .unwrap();
        assert!(bare_about.html.is_empty());
        assert_eq!(bare_about.layout, "page");
        assert_eq!(bare_about.title.as_deref(), Some("About Page"));
        assert_eq!(
            bare_about.tags,
            Some(vec!["one".to_string(), "two".to_string()])
        );
        assert_eq!(bare_about.extra.as_ref().unwrap()["custom"], "x");

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn liquid_collections_must_live_inside_entry_dir() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("liquidoutside")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: default\n---\n# Home\n",
        );
        write_layout(&tmp.join("contents"), "default");
        fs::create_dir("outside")?;
        fs::write("outside/evil.html", "<p>no</p>")?;

        // Outside the entry directory → InvalidInput.
        let outside = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            liquid_collections: Some(vec!["../outside".into()]),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => {
                return Err(io::Error::other(
                    "outside liquid collection unexpectedly succeeded",
                ));
            }
        };
        assert_eq!(outside.kind(), io::ErrorKind::InvalidInput);

        // Missing collection → NotFound.
        let missing = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            liquid_collections: Some(vec!["_missing".into()]),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => {
                return Err(io::Error::other(
                    "missing liquid collection unexpectedly succeeded",
                ));
            }
        };
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);

        // A non-liquid file inside the entry dir → InvalidInput.
        fs::write("contents/notes.txt", "plain")?;
        let non_liquid = match BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            liquid_collections: Some(vec!["notes.txt".into()]),
            ..Default::default()
        }) {
            Err(error) => error,
            Ok(_) => {
                return Err(io::Error::other(
                    "non-liquid collection unexpectedly succeeded",
                ));
            }
        };
        assert_eq!(non_liquid.kind(), io::ErrorKind::InvalidInput);

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn watcher_tracks_new_and_removed_liquid_files() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("liquidwatch")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: default\n---\n# Home\n",
        );
        write_layout(&tmp.join("contents"), "default");
        fs::create_dir_all(tmp.join("contents/_pages"))?;
        fs::write(tmp.join("contents/_pages/a.html"), "<p>A</p>")?;

        let mut cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            liquid_collections: Some(vec!["_pages".into()]),
            ..Default::default()
        })?;
        cache.start_watching()?;
        assert_eq!(cache.liquid_files().len(), 1);

        // Create a new liquid file inside the collection.
        let new_path = tmp.join("contents/_pages/b.html");
        fs::write(&new_path, "<p>B</p>")?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if cache.liquid_files().iter().any(|p| p.ends_with("b.html")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(cache.liquid_files().iter().any(|p| p.ends_with("b.html")));
        assert!(cache.get(&new_path).is_some());

        // Remove it again.
        fs::remove_file(&new_path)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !cache.liquid_files().iter().any(|p| p.ends_with("b.html")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !cache.liquid_files().iter().any(|p| p.ends_with("b.html")),
            "removed liquid file remained tracked"
        );
        assert!(cache.get(&new_path).is_none());

        cache.stop_watching();
        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn lru_eviction_falls_through_to_archive() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("evict")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "docs/index.md",
            "---\nlayout: default\n---\n# Small\n",
        );
        write_md(
            &tmp,
            "docs/big.md",
            "---\nlayout: default\n---\n# Big\nBody padding to exceed the budget.\n",
        );
        write_layout(&tmp.join("docs"), "default");

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("docs".into()),
            memory: Some(MemoryConfig {
                byte_budget: Some(10),
                entry_budget: None,
            }),
            // Tiny thresholds so the flush happens quickly.
            archive: Some(ArchiveConfig {
                shard_bytes: Some(1),
                flush_interval_ms: Some(100),
            }),
            ..Default::default()
        })?;
        cache.flush();

        // The LRU evicted entries, but they are still readable (archive).
        let files = cache.markdown_files();
        assert_eq!(files.len(), 2, "both files still resolvable");
        for f in &files {
            assert!(cache.get_text(f.clone()).is_some(), "{} readable", f);
        }

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn resolve_url_uses_hosting_config() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("resolve")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "contents/index.md",
            "---\nlayout: default\n---\n# Home\n",
        );
        write_layout(&tmp.join("contents"), "default");

        let cache = BaganCache::with_config(BaganCacheConfig {
            entry_dir: Some("contents".into()),
            hosting: Some(HostingConfig {
                provider: Some(HostingProvider::GithubPages),
                repo: Some("my-repo".into()),
                ..Default::default()
            }),
            ..Default::default()
        })?;
        assert_eq!(cache.base_url(), "/my-repo/");
        assert_eq!(
            cache.resolve_url("docs/intro".into()),
            "/my-repo/docs/intro"
        );

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn gitignore_entry_not_duplicated() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("gitignore")?;
        env::set_current_dir(&tmp)?;
        fs::write(".gitignore", format!("{DEFAULT_ARCHIVE_DIR_NAME}\n"))?;
        write_md(&tmp, "docs/index.md", "---\nlayout: default\n---\n# Home\n");
        write_md(&tmp, "docs/doc2.md", "# No frontmatter\nContent\n");
        write_layout(&tmp.join("docs"), "default");

        let _cache = cache_for("docs")?;

        let git = fs::read_to_string(".gitignore")?;
        let cache_dir_occurrences = git
            .lines()
            .filter(|l| l.trim() == DEFAULT_ARCHIVE_DIR_NAME)
            .count();
        let out_dir_occurrences = git.lines().filter(|l| l.trim() == DEFAULT_OUT_DIR).count();
        assert_eq!(cache_dir_occurrences, 1);
        assert_eq!(out_dir_occurrences, 1);

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn map_json_contains_frontmatter() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("mapjson")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "docs/index.md", "---\nlayout: default\n---\n# Home\n");
        write_md(
            &tmp,
            "docs/tagged.md",
            "---\ntitle: Tagged\nlayout: default\ntags:\n  - x\n---\n# Tagged\n",
        );
        write_layout(&tmp.join("docs"), "default");

        let cache = cache_for("docs")?;
        cache.flush();

        let map: serde_json::Value = serde_json::from_str(&cache.map_json()).unwrap();
        let entry = &map["files"]["docs/tagged.md"];
        assert_eq!(entry["frontmatter"]["title"], "Tagged");
        assert_eq!(entry["frontmatter"]["tags"][0], "x");
        assert_eq!(entry["kind"], "text");
        assert!(entry["hash"].as_str().unwrap().len() == 64);

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn pretty_path_renders_relative_to_root() {
        let root = Path::new("/project");
        assert_eq!(
            pretty_path(Path::new("/project/contents/docs/a.md"), root),
            "contents/docs/a.md"
        );
        // Root-level paths render as their bare name.
        assert_eq!(
            pretty_path(Path::new("/project/README.md"), root),
            "README.md"
        );
        // Paths outside the root fall back to the absolute path.
        assert_eq!(
            pretty_path(Path::new("/elsewhere/b.png"), root),
            "/elsewhere/b.png"
        );
    }

    #[test]
    fn format_change_line_matches_kind_and_relative_path() {
        let root = Path::new("/project");
        let abs = root.join("contents").join("a.md");
        assert_eq!(
            format_change_line("create", &abs, root, false),
            "created contents/a.md"
        );
        assert_eq!(
            format_change_line("modify", &abs, root, false),
            "modified contents/a.md"
        );
        assert_eq!(
            format_change_line("remove", &abs, root, false),
            "removed contents/a.md"
        );
        assert_eq!(
            format_change_line("other", &abs, root, false),
            "changed contents/a.md"
        );
        // Colors wrap the label only; the path stays plain.
        let colored = format_change_line("modify", &abs, root, true);
        assert_eq!(
            colored,
            format!("{LOG_STYLE_YELLOW}modified{LOG_STYLE_RESET} contents/a.md")
        );
        // Paths outside the root fall back to the absolute path.
        let outside = Path::new("/elsewhere/b.png");
        assert_eq!(
            format_change_line("create", outside, root, false),
            "created /elsewhere/b.png"
        );
    }

    #[test]
    fn classify_change_uses_filesystem_state() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("classify")?;
        env::set_current_dir(&tmp)?;
        write_md(
            &tmp,
            "docs/index.md",
            "---\nlayout: default\n---\n# Known\n",
        );
        write_layout(&tmp.join("docs"), "default");

        let cache = cache_for("docs")?;

        // Exists + known to the cache → modify.
        assert_eq!(
            classify_change(
                &cache.memory,
                &cache.archive,
                &tmp.join("docs").join("index.md")
            ),
            "modify"
        );
        // Exists + unknown → create.
        write_md(&tmp, "docs/fresh.md", "# Fresh\n");
        assert_eq!(
            classify_change(
                &cache.memory,
                &cache.archive,
                &tmp.join("docs").join("fresh.md")
            ),
            "create"
        );
        // Gone → remove (even though never cached).
        assert_eq!(
            classify_change(
                &cache.memory,
                &cache.archive,
                &tmp.join("docs").join("ghost.md")
            ),
            "remove"
        );

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn should_log_change_one_line_per_operation() {
        let now = Instant::now();
        let p = Path::new("/project/contents/a.md");
        let mut last: HashMap<PathBuf, Instant> = HashMap::new();

        // Untracked extensions (editor side files) never log.
        let tmp = Path::new("/project/contents/a.md.tmp");
        assert!(!should_log_change(tmp, &last, now));
        // A trackable path with no recent line logs; the duplicate
        // events of one operation (create/remove/modify within the
        // window) do not log again.
        assert!(should_log_change(p, &last, now));
        last.insert(p.to_path_buf(), now);
        assert!(!should_log_change(p, &last, now));
        // A second operation 2 s later is a new save — log it.
        let later = now + Duration::from_secs(2);
        assert!(should_log_change(p, &last, later));
    }

    #[test]
    fn log_trackable_matches_cache_admission() {
        // Markdown and asset extensions log; editor side files never.
        assert!(log_trackable(Path::new("a/hi.md")));
        assert!(log_trackable(Path::new("a/hi.markdown")));
        assert!(log_trackable(Path::new("a/logo.png")));
        assert!(log_trackable(Path::new("a/style.css")));
        assert!(!log_trackable(Path::new("a/hi.md.tmp")));
        assert!(!log_trackable(Path::new("a/hi.tmp")));
        assert!(!log_trackable(Path::new("a/.hi.md.swp")));
        assert!(!log_trackable(Path::new("a/hi")));
    }

    #[test]
    fn stop_watching_is_safe_without_starting() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("stopwatch")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "docs/index.md", "---\nlayout: default\n---\n# A\n");
        write_layout(&tmp.join("docs"), "default");

        let mut cache = cache_for("docs")?;
        // Stop without start, then stop twice — all must be no-ops.
        cache.stop_watching();
        cache.start_watching()?;
        cache.stop_watching();
        cache.stop_watching();
        // The watcher is gone: the entry stayed in memory throughout.
        assert!(cache.get(&tmp.join("docs").join("index.md")).is_some());

        restore(orig, tmp);
        Ok(())
    }

    #[test]
    fn watcher_removes_deleted_files_from_cache() -> io::Result<()> {
        let _guard = TEST_CWD_LOCK.lock().unwrap();
        let (tmp, orig) = make_tmp("watchremove")?;
        env::set_current_dir(&tmp)?;
        write_md(&tmp, "docs/index.md", "---\nlayout: default\n---\n# Home\n");
        write_md(&tmp, "docs/a.md", "---\nlayout: default\n---\n# A\n");
        write_layout(&tmp.join("docs"), "default");

        let mut cache = cache_for("docs")?;
        let path = tmp.join("docs/a.md");
        cache.start_watching()?;
        fs::remove_file(&path)?;

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if cache.get(&path).is_none() {
                cache.stop_watching();
                restore(orig, tmp);
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        cache.stop_watching();
        restore(orig, tmp);
        Err(io::Error::other("deleted file remained in the cache"))
    }
}
