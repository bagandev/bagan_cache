//! Memory-first cache of a site's Markdown and asset files.

// cspell:disable

// fn with_config_inner(env: Env, config: BaganCacheOptions) -> io::Result<Self> {
mod archive;
mod base_url;
mod def;
mod file;
mod log;
mod lru;
mod obj_cache;
mod theme;
mod utils;

pub use archive::ArchiveHandle;
pub use base_url::{BaseDirInfo, HostingProvider};
use def::{DEFAULT_ARCHIVE_DIR_NAME, DEFAULT_OUT_DIR, NODE_MODULES_DIR};
pub use lru::LruCache;
pub use obj_cache::{
    AssetsObject, LiquidObject, ObjectCache, PageObject, asset_file_type, asset_out_file,
    asset_slug, extra_frontmatter_fields, file_name_no_ext, liquid_file_name_no_ext,
    liquid_out_file, liquid_slug, markdown_slug, page_out_file,
};
use theme::{BaganThemeConfig, ThemeDirs};

use file::{
    ASSET_BINARY_EXTENSIONS, ASSET_EXTENSIONS, LIQUID_EXTENSIONS, MARKDOWN_EXTENSIONS,
    collect_asset_files, collect_markdown_files, has_liquid_extension,
};
use log::{classify_change, log_change_pretty, should_log_change};
use napi::bindgen_prelude::*;
use napi::threadsafe_function::ThreadsafeFunction;
use napi_derive::napi;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use peisar::plugin::Plugin;
use peisar::{
    PeisarOptions, RenderOptions,
    ast::{AstOptions, md_to_ast_with_hooks},
    get_options,
    plugin::{ParserHooks, RegisteredPlugin, visit_document_mut},
    render_document_html,
};
use utils::{ArchiveConfig, CacheChangeEvent, CachedContent, HostingConfig, MemoryConfig};

use peisar_yaml::frontmatter;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
pub type LiquidRenderer = FunctionRef<String, serde_json::Value>;
/// JavaScript callback type used by the options object. A `FunctionRef`
/// cannot be converted directly from/to an N-API value.
pub type LiquidRenderFunction = Function<'static, String, serde_json::Value>;

#[napi(object)]
pub struct BaganCacheOptions {
    pub entry_dir: Option<String>,
    pub assets_dir: Option<String>,
    pub memory: Option<MemoryConfig>,
    /// Theme package name (Jekyll-gem style), resolved from
    /// `node_modules/<theme>`. Omit (null/undefined) for a local site:
    /// the theme directories below are then resolved from the entry
    /// directory.
    pub theme: Option<String>,
    /// Local layouts directory (local mode only), relative to the entry
    /// directory. Defaults to `_layouts`.
    pub theme_layouts_dir: Option<String>,
    /// Local includes directory (local mode only), relative to the entry
    /// directory. Defaults to `_includes`.
    pub theme_includes_dir: Option<String>,
    /// Local theme assets directory (local mode only), relative to the
    /// entry directory. Defaults to `_assets`.
    pub theme_assets_dir: Option<String>,
    /// Local data directory (local mode only), relative to the entry
    /// directory. Defaults to `_data`.
    pub theme_data_dir: Option<String>,
    /// Theme configuration object (`themeConfigObject`). When omitted
    /// (and `theme` is set) it is loaded automatically from the theme
    /// package's default export.
    pub theme_config: Option<BaganThemeConfig>,
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
    pub liquid_render: Option<LiquidRenderFunction>,
}

/// Boxed change callback held in the subscription registry.
///
/// In npm builds, JavaScript callbacks are wrapped into a thread-safe
/// function before they are stored because watcher events fire off-thread.
pub type ChangeCallback = Box<dyn Fn(CacheChangeEvent) + Send + 'static>;

/// Event-loop keep-alive held while the watcher runs (npm builds): a
/// strong (ref'd) threadsafe function, so Node keeps running as long as
/// watching is active — even for a plain script with nothing else pending.
///
/// `CalleeHandled = false` matches what
/// [`Function::build_threadsafe_function`] produces.
pub type KeepAlive = ThreadsafeFunction<(), Unknown<'static>, (), Status, false>;

// Backward-compatible name for callbacks used by the JavaScript binding.
// pub type JsChangeCallback = CacheChangeCallback;

// Shared registry of change callbacks, keyed by subscription id.
#[derive(Clone, Default)]
struct ChangeCallbacks {
    inner: Arc<Mutex<HashMap<u32, ChangeCallback>>>,
    next_id: Arc<AtomicU32>,
}

impl ChangeCallbacks {
    fn add(&self, cb: ChangeCallback) -> u32 {
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

/// Build the event-loop keep-alive held while the watcher runs: a strong
/// (ref'd, i.e. NOT `.weak()`) threadsafe function over a no-op JS
/// function. The ref'd TSFN is visible to libuv, so Node keeps the
/// process alive as long as the handle exists; dropping it unrefs the
/// loop again and lets the process exit.

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

/// Load the theme configuration from a theme package's `package.json`
/// (name/version/author metadata).
///
/// This is the fallback for Rust consumers that have no JS runtime to
/// evaluate the package's default export; the napi constructor loads the
/// real `themeConfigObject` from the export itself.
fn theme_config_from_package_json(theme_path: &Path) -> BaganThemeConfig {
    let mut config = BaganThemeConfig::default();
    if let Ok(pkg_text) = fs::read_to_string(theme_path.join("package.json"))
        && let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&pkg_text)
    {
        if let Some(name) = pkg.get("name").and_then(serde_json::Value::as_str) {
            config.name = Some(name.to_string());
        }
        if let Some(version) = pkg.get("version").and_then(serde_json::Value::as_str) {
            config.version = Some(version.to_string());
        }
        if let Some(author) = pkg.get("author").and_then(|a| match a {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) => o
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        }) {
            config.author = Some(author);
        }
    }
    config
}

/// Load the `themeConfigObject` a theme package exports as its default
/// export through the JS runtime (`node_modules/<theme>`).
///
/// Runs on the current JS thread — must only be called from a napi
/// constructor/factory, never from a background thread. Returns `Ok(None)`
/// when the package cannot be required (pure asset themes without a JS
/// entry are a supported setup); the require failure is caught inside
/// the script so no pending JS exception is left on the env.
fn load_theme_config_from_module(
    env: &Env,
    cwd: &Path,
    theme_name: &str,
) -> Result<Option<BaganThemeConfig>> {
    // Anchor the loader in the project root so `node_modules/<theme>`
    // resolves exactly like it would for the user's own code. A synthetic
    // filename (not a real file) keeps `pathToFileURL` happy without
    // requiring one to exist. `process.getBuiltinModule` is used because
    // `napi_run_script` has no module scope (no `require`); it exists in
    // Node ≥ 20.16 / ≥ 22.3, within the supported engine range.
    let anchor = cwd.join("index.js").to_string_lossy().to_string();
    let script = format!(
        "(() => {{ try {{ \
         const {{ createRequire }} = process.getBuiltinModule('node:module'); \
         const {{ pathToFileURL }} = process.getBuiltinModule('node:url'); \
         const req = createRequire(pathToFileURL({anchor}).href); \
         const mod = req({theme_json}); \
         return mod && mod.default !== undefined ? mod.default : mod; \
         }} catch {{ return null; }} }})()",
        anchor = serde_json::to_string(&anchor).unwrap_or_default(),
        theme_json = serde_json::to_string(theme_name).unwrap_or_default(),
    );
    let theme_default: Option<BaganThemeConfig> = env.run_script(script)?;
    Ok(theme_default)
}
/// In-memory + archive-backed cache of the Markdown and asset files under a
/// directory tree.
///
/// Memory is the source of truth for hot content; a background worker
/// flushes a binary shard archive to `.peisar-cache` for cold/large
/// entries, front-ended by the `map.json` search index. Watchers keep the
/// memory tier in sync with the filesystem.
#[napi]
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
    watch_keep_alive: Option<KeepAlive>,
    entry_dir: PathBuf,
    assets_dir: Option<PathBuf>,
    theme_path: PathBuf,
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
    /// JS-side `liquidRender(file)` callback for LiquidObject values
    /// (npm builds hold a `FunctionRef` + the napi `Env` captured at
    /// construction; rendering runs on the JS main thread only). The
    /// callback returns the front-matter fields + rendered HTML.
    liquid_renderer: Option<LiquidRenderer>,
    /// The napi `Env` captured at construction, used to `borrow_back` the
    /// `liquidRenderer` callback (npm builds only).
    liquid_env: Option<Env>,
}
#[napi]
impl BaganCache {
    /// JS: `new BaganCache(entryDir?, assetsDir?, options?)`.
    ///
    /// `options` configures the theme, Markdown parsing (`fragment`, GFM,
    /// …), archive thresholds, hosting resolution, and output directories.
    /// In theme mode `options.themeConfig` is the theme package's default
    /// export (`themeConfigObject`); the caller is expected to `require`
    /// (or import) the theme package and pass the export through. Register
    /// custom visitors and parser hooks afterwards with `usePlugin()`.
    #[napi(constructor)]
    pub fn new(env: Env, mut options: Option<BaganCacheOptions>) -> Result<Self> {
        let mut config = BaganCacheOptions {
            entry_dir: options.as_ref().and_then(|o| o.entry_dir.clone()),
            assets_dir: options.as_ref().and_then(|o| o.assets_dir.clone()),
            theme: options.as_ref().and_then(|o| o.theme.clone()),
            theme_layouts_dir: options.as_ref().and_then(|o| o.theme_layouts_dir.clone()),
            theme_includes_dir: options.as_ref().and_then(|o| o.theme_includes_dir.clone()),
            theme_assets_dir: options.as_ref().and_then(|o| o.theme_assets_dir.clone()),
            theme_data_dir: options.as_ref().and_then(|o| o.theme_data_dir.clone()),
            theme_config: options.as_ref().and_then(|o| o.theme_config.clone()),
            markdown: options.as_ref().and_then(|o| o.markdown.clone()),
            archive: options.as_ref().and_then(|o| o.archive),
            hosting: options.as_ref().and_then(|o| o.hosting.clone()),
            out_dir: options.as_ref().and_then(|o| o.out_dir.clone()),
            cache_dir: options.as_ref().and_then(|o| o.cache_dir.clone()),
            liquid_collections: options.as_ref().and_then(|o| o.liquid_collections.clone()),
            memory: None,
            liquid_render: options.as_mut().and_then(|o| o.liquid_render.take()),
        };
        if let Some(theme_config) = options.as_ref().and_then(|o| o.theme_config.clone()) {
            config.theme_config = Some(theme_config);
        } else if let Some(theme_name) = config
            .theme
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            // Auto-load the theme package's default export
            // (`themeConfigObject`) through the JS runtime. The theme
            // must be installed as `node_modules/<theme>` relative to the
            // cwd, which is the same place `with_config` resolves it.
            // Pure asset themes (layouts/templates only, no JS entry)
            // report `None` here — also a supported setup; the load
            // then falls back to the package's package.json metadata
            // inside `with_config`.
            let cwd = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            config.theme_config = load_theme_config_from_module(&env, &cwd, theme_name)?;
        }
        BaganCache::with_config(env, config)
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, format!("{e}")))
    }
    pub fn with_config(env: Env, config: BaganCacheOptions) -> Result<Self> {
        Self::with_config_inner(env, config)
            .map_err(|e| napi::Error::new(napi::Status::GenericFailure, format!("{e}")))
    }
    fn with_config_inner(env: Env, config: BaganCacheOptions) -> io::Result<Self> {
        let cwd = env::current_dir()?;
        // Resolve the entry dir to an absolute path first: watcher events
        // carry absolute paths, so the `is_markdown` prefix check needs an
        // absolute entry_dir even when JS passes a relative one — and in
        // local mode the theme directories below resolve from it.
        let entry_dir = {
            let d = Path::new(config.entry_dir.as_deref().unwrap_or("."));
            if d.is_absolute() {
                d.to_path_buf()
            } else {
                cwd.join(d)
            }
        };
        if !MARKDOWN_EXTENSIONS
            .iter()
            .any(|ext| entry_dir.join(format!("index.{ext}")).is_file())
        {
            let message = format!(
                "entry directory '{}' must contain an index file with a supported Markdown extension",
                entry_dir.display()
            );
            eprintln!("BaganCache configuration error: {message}");
            return Err(io::Error::new(io::ErrorKind::NotFound, message));
        }
        let entry_dir = fs::canonicalize(&entry_dir)?;
        // Resolve the theme (Jekyll-gem style):
        // - `theme` set → `node_modules/<theme>`, configuration from the
        //   package's default export (`themeConfigObject`).
        // - `theme` unset → local site; `_layouts`/`_includes`/`_assets`/
        //   `_data` (or the configured overrides) resolved from the
        //   entry directory.
        let theme_name = config
            .theme
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let (theme_path, theme_config) = match theme_name {
            Some(theme_name) => {
                let theme_name_path = Path::new(theme_name);
                if theme_name_path.is_absolute()
                    || theme_name_path.components().any(|component| {
                        matches!(
                            component,
                            std::path::Component::ParentDir | std::path::Component::RootDir
                        )
                    })
                {
                    let message = format!("invalid theme package name '{theme_name}'");
                    eprintln!("BaganCache configuration error: {message}");
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, message));
                }
                let theme_path = cwd.join(NODE_MODULES_DIR).join(theme_name_path);
                if !theme_path.is_dir() {
                    let message = format!(
                        "theme package '{}' does not exist or is not a directory",
                        theme_path.display()
                    );
                    eprintln!("BaganCache configuration error: {message}");
                    return Err(io::Error::new(io::ErrorKind::NotFound, message));
                }
                // The `themeConfigObject` the package exports as its
                // default (loaded by the napi constructor through the
                // JS runtime), or — for JS-less Rust consumers — the
                // package.json metadata fallback.
                let mut theme_config = config
                    .theme_config
                    .clone()
                    .unwrap_or_else(|| theme_config_from_package_json(&theme_path));
                theme_config
                    .name
                    .get_or_insert_with(|| theme_name.to_string());
                (theme_path, theme_config)
            }
            None => {
                // Local mode: the theme directories live inside the
                // entry directory, which doubles as the theme root.
                let theme_config = BaganThemeConfig {
                    layouts_dir: config.theme_layouts_dir.clone(),
                    includes_dir: config.theme_includes_dir.clone(),
                    assets_dir: config.theme_assets_dir.clone(),
                    data_dir: config.theme_data_dir.clone(),
                    ..Default::default()
                };
                (entry_dir.clone(), theme_config)
            }
        };
        let theme_assets_dir = theme_path.join(theme_config.assets_dir_or_default());
        let theme_layouts_dir = theme_path.join(theme_config.layouts_dir_or_default());
        let theme_includes_dir = theme_path.join(theme_config.includes_dir_or_default());
        let theme_data_dir = theme_path.join(theme_config.data_dir_or_default());
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
        let (liquid_files, liquid_roots): (HashMap<PathBuf, PathBuf>, Vec<PathBuf>) = {
            let mut roots: Vec<PathBuf> = Vec::new();
            let mut files: HashMap<PathBuf, PathBuf> = HashMap::new();
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
            watch_keep_alive: None,
            entry_dir,
            assets_dir: resolved_assets,
            theme_path,
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
            liquid_renderer: config
                .liquid_render
                .and_then(|render| render.create_ref().ok()),
            liquid_env: Some(env),
        })
    }

    /// Register a JavaScript plugin applied to every page render
    /// (JS: `cache.usePlugin({ visitBlock, visitInline, parseBlock, parseInline })`).
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
    /// the JS main thread every time `liquids()` / `getLiquid()` builds
    /// a [`LiquidObject`]; a callback that throws or returns nothing
    /// leaves the front-matter fields empty and `html` as the raw
    /// template. Also accepts the callback through the constructor
    /// option `liquidRender`.
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
    /// JS: `cache.startWatchingJs()` — start watching the entry (and
    /// assets) directories recursively; errors surface as JS exceptions.
    ///
    /// While watching is active this holds a strong (ref'd) threadsafe
    /// function on the Node event loop, so a plain script that only calls
    /// `startWatchingJs()` keeps running until `dispose()` (or process
    /// exit) stops the watcher. Without it, the native watcher thread is
    /// invisible to libuv and Node exits as soon as the script ends.
    #[napi]
    pub fn start_watching(&mut self, env: Env) -> Result<()> {
        self.start_watching_local()
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
    pub fn start_watching_local(&mut self) -> io::Result<()> {
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
    #[napi]
    pub fn get_text(&self, abs_path: String) -> Option<String> {
        self.get(Path::new(&abs_path)).and_then(|c| match c {
            CachedContent::Text(s) => Some(s),
            _ => None,
        })
    }

    /// JS: `cache.getBinary(absPath)` — cached bytes of a binary asset, or
    /// `null`. Reads fall through to the archive tier.
    #[napi]
    pub fn get_binary(&self, abs_path: String) -> Option<Vec<u8>> {
        self.get(Path::new(&abs_path)).and_then(|c| match c {
            CachedContent::Binary(b) => Some(b),
            _ => None,
        })
    }

    /// JS: `cache.listFiles()` — absolute paths of everything cached
    /// (memory + archive tiers).
    #[napi]
    pub fn list_files(&self) -> Vec<String> {
        self.all_keys()
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    /// JS: `cache.markdownFiles()` — absolute paths of cached markdown
    /// files (either tier).
    #[napi]
    pub fn markdown_files(&self) -> Vec<String> {
        self.filter_cached_files(|ext| MARKDOWN_EXTENSIONS.contains(&ext))
    }

    /// Accepted Markdown layouts discovered from the theme's `.html`
    /// templates, sorted by name.
    ///
    /// JS: `cache.markdownLayouts()`.
    #[napi]
    pub fn markdown_layouts(&self) -> Vec<String> {
        let mut layouts: Vec<String> = self.markdown_layouts.iter().cloned().collect();
        layouts.sort();
        layouts
    }

    /// Absolute paths to the configured theme's layouts, assets, and data
    /// directories.
    #[napi]
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
    #[napi]
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

    /// Rust: every page object, sorted by slug.
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
        pages.sort_by(|a, b| a.slug.cmp(&b.slug));
        pages
    }

    /// JS: `cache.pages()` — every page object, for Liquid themes
    /// (`{{ page.title }}`, `{{ page.layout }}`, …).
    #[napi]
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
    /// input path, or `null`.
    #[napi]
    pub fn get_page(&self, input_path: String) -> Option<PageObject> {
        self.page_object_for(Path::new(&input_path))
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
    #[napi]
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
    #[napi]
    pub fn get_asset(&self, input_path: String) -> Option<AssetsObject> {
        self.asset_object_for(Path::new(&input_path))
    }

    // ===== Liquid collection objects (rendered on the JS side) =====

    /// Rust: every liquid object, sorted by slug.
    pub fn liquid_objects(&self) -> Vec<LiquidObject> {
        let tracked = self.liquid_files.lock().unwrap().clone();
        let mut liquids: Vec<LiquidObject> = tracked
            .iter()
            .filter_map(|(path, base)| self.liquid_object_for_with_base(path, base))
            .collect();
        liquids.sort_by(|a, b| a.slug.cmp(&b.slug));
        liquids
    }

    /// JS: `cache.liquids()` — every liquid object for the configured
    /// `liquidCollections`: front-matter fields from the JS-side
    /// `liquidRender(file)` callback (which also renders the HTML),
    /// path-derived fields from the cache.
    #[napi]
    pub fn liquids(&self) -> Vec<LiquidObject> {
        self.liquid_objects()
    }

    /// Build one [`LiquidObject`] for a liquid template path (either
    /// tier), slugging relative to its collection base directory: files
    /// from a directory collection slug relative to the collection root,
    /// single-file collections relative to the entry directory.
    pub fn liquid_object_for(&self, path: &Path) -> Option<LiquidObject> {
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
    fn liquid_object_for_with_base(&self, path: &Path, base: &Path) -> Option<LiquidObject> {
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
        let fallback = LiquidObject {
            layout: fallback_layout
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            title: fm_string(fm_ref, &["title"]),
            summary: fm_string(fm_ref, &["summary", "description"]),
            tags: fm_string_list(fm_ref, "tags"),
            publish_date: fm_string(fm_ref, &["date", "publishDate", "publish_date"]),
            extra: fm_ref.and_then(extra_frontmatter_fields),
            html: String::new(),
            file_name: String::new(),
            slug: String::new(),
            raw: String::new(),
            out_file_path: String::new(),
            input_file_path: String::new(),
        };
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
            .or(fallback.title);
        let summary = rendered
            .as_ref()
            .and_then(|r| r.summary.clone())
            .or(fallback.summary);
        let tags = rendered
            .as_ref()
            .and_then(|r| r.tags.clone())
            .or(fallback.tags);
        let publish_date = rendered
            .as_ref()
            .and_then(|r| r.publish_date.clone())
            .or(fallback.publish_date);
        let extra = rendered
            .as_ref()
            .and_then(|r| r.extra.clone())
            .or(fallback.extra);
        let html = rendered.map(|r| r.html).unwrap_or_default();
        let base_url = self.hosting.base_url();
        Some(LiquidObject {
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

    /// JS: `cache.getLiquid(inputPath)` — a single liquid object by
    /// absolute input path, or `null`.
    #[napi]
    pub fn get_liquid(&self, input_path: String) -> Option<LiquidObject> {
        self.liquid_object_for(Path::new(&input_path))
    }

    /// JS: `cache.liquidFiles()` — absolute paths of the tracked liquid
    /// template files (either tier).
    #[napi]
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
    fn render_liquid(&self, raw: &str) -> Option<LiquidObject> {
        let renderer = &self.liquid_renderer;
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
            Some(LiquidObject {
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
    }

    /// Absolute path to the configured theme package — or, in local
    /// mode (no `theme` configured), the entry directory the local
    /// theme directories resolve from.
    #[napi]
    pub fn theme_path(&self) -> String {
        self.theme_path.to_string_lossy().to_string()
    }

    /// Parsed theme metadata exposed as an object to Rust and JavaScript.
    #[napi]
    pub fn theme_config(&self) -> BaganThemeConfig {
        self.theme_config.clone()
    }

    // ===== Archive-tier introspection =====

    /// JS: `cache.archived()` — absolute paths flushed to the disk
    /// archive. Everything not listed still lives in memory only.
    #[napi]
    pub fn archived(&self) -> Vec<String> {
        self.archive
            .keys()
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect()
    }

    /// JS: `cache.mapJson()` — the current `map.json` document (path →
    /// shard location + metadata, including front matter for search).
    #[napi]
    pub fn map_json(&self) -> String {
        self.archive.map_json()
    }

    /// JS: `cache.flush()` — write all pending archive entries now,
    /// blocking until the worker is done.
    #[napi]
    pub fn flush(&self) {
        self.archive.flush();
    }

    // ===== Hosting / baseUrl =====

    /// JS: `cache.baseUrl()` — the resolved base URL path prefix for the
    /// configured hosting (e.g. `/repo/` for a GitHub Pages project site).
    #[napi]
    pub fn base_url(&self) -> String {
        self.hosting.base_url()
    }

    /// JS: `cache.resolveUrl(path)` — prefix a site-relative path with the
    /// resolved base URL: `resolveUrl("docs/intro")` → `/repo/docs/intro`.
    #[napi]
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
    #[napi]
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

    /// JS: `cache.offChange(id)` — remove a previously registered callback.
    #[napi]
    pub fn off_change(&self, id: u32) {
        self.change_callbacks.remove(id);
    }

    /// JS: `cache.dispose()` — stop the watcher, flush pending archive
    /// writes, and drop JS change callbacks. Safe to call more than once.
    ///
    /// Rust consumers do not need this: dropping the [`BaganCache`] stops
    /// the watcher, flushes the archive, and joins the workers.
    #[napi]
    pub fn dispose(&mut self) {
        self.stop_watching();
        self.change_callbacks.clear();
    }
}

// Dropping a `BaganCache` is the Rust equivalent of the JS `dispose()`:
// the watcher is released with the struct, the archive worker flushes its
// pending buffer and exits, and the memory tier goes away with it. In npm
// builds the event-loop keep-alive goes with it too, so a Node process
// holding only the cache can exit once the cache is garbage-collected.
impl Drop for BaganCache {
    fn drop(&mut self) {
        self.watcher = None;
        {
            self.watch_keep_alive = None;
        }
        self.change_callbacks.clear();
        self.archive.close();
    }
}
