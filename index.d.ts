// Generated from ci.yml
// napi-rs helper types 
export type PeisarEmphasisLevel = "Italic" | "Bold";
export type PeisarTableCellAlignment = "Default" | "Left" | "Center" | "Right";
export type PeisarTableCellAlignments = PeisarTableCellAlignment[];
export type PeisarTaskState = "Unchecked" | "Checked";
export type BaganHostingProvider =
  | "GithubPages"
  | "Vercel"
  | "Netlify"
  | "CloudflarePages"
  | "Custom";

/**
 * Which binding artifact the generated loader actually loaded: `'native'` for
 * a native addon, otherwise the `platformArchABI` of the WASI flavor. Every
 * flavor napi-rs can build is listed, because `NAPI_RS_NATIVE_LIBRARY_PATH`
 * can point the loader at a WASI artifact this package does not build itself.
 */
export declare const __napiBindingTarget: 'native' | 'wasm32-wasi' | 'wasm32-wasip1'

/**
 * In-memory + archive-backed cache of the Markdown and asset files under a
 * directory tree.
 *
 * Memory is the source of truth for hot content; a background worker
 * flushes a binary shard archive to `.peisar-cache` for cold/large
 * entries, front-ended by the `map.json` search index. Watchers keep the
 * memory tier in sync with the filesystem.
 */
export declare class BaganCache {
  /**
   * JS: `new BaganCache(entryDir?, assetsDir?, options?)`.
   *
   * `options` configures the theme, Markdown parsing (`fragment`, GFM,
   * …), archive thresholds, hosting resolution, and output directories.
   * In theme mode `options.themeConfig` is the theme package's default
   * export (`themeConfigObject`); the caller is expected to `require`
   * (or import) the theme package and pass the export through. Register
   * custom visitors and parser hooks afterwards with `usePlugin()`.
   */
  constructor(entryDir?: string | undefined | null, assetsDir?: string | undefined | null, options?: BaganCacheJsOptions | undefined | null)
  /**
   * JS: `BaganCache.withConfigJs(config)` — full configuration form,
   * including memory-tier budgets. Liquid collections may be
   * configured through `liquidCollections`, but `liquidRender` is only
   * available on the constructor (the `withConfig` object cannot carry
   * functions across the boundary) — use `useLiquidRender()` to
   * register the callback afterwards.
   */
  static withConfigJs(config: BaganCacheConfig): BaganCache
  /**
   * Register a JavaScript plugin applied to every page render
   * (JS: `cache.usePlugin({ visitBlock, visitInline, parseBlock, parseInline })`).
   */
  usePlugin(plugin: Plugin): void
  /**
   * JS: `cache.useLiquidRender(cb)` — register the JavaScript function
   * that renders a Liquid collection file to a page
   * (`liquidRender(file: string) => PageObject`). The callback
   * extracts the front matter, renders the Liquid content to HTML,
   * and returns the page fields (converted leniently — `html` is the
   * meaningful field, anything else falls back to the raw front
   * matter); the cache fills the path-derived fields (fileName,
   * slug, outFilePath, inputFilePath) and `raw` itself. Called on
   * the JS main thread every time `liquids()` / `getLiquid()` builds
   * a [`LiquidObject`]; a callback that throws or returns nothing
   * leaves the front-matter fields empty and `html` as the raw
   * template. Also accepts the callback through the constructor
   * option `liquidRender`.
   */
  useLiquidRender(render: (file: string) => PageObject): void
  /**
   * JS: `cache.startWatchingJs()` — start watching the entry (and
   * assets) directories recursively; errors surface as JS exceptions.
   *
   * While watching is active this holds a strong (ref'd) threadsafe
   * function on the Node event loop, so a plain script that only calls
   * `startWatchingJs()` keeps running until `dispose()` (or process
   * exit) stops the watcher. Without it, the native watcher thread is
   * invisible to libuv and Node exits as soon as the script ends.
   */
  startWatchingJs(): void
  /**
   * JS: `cache.getText(absPath)` — cached text of a file, or `null`.
   * Reads fall through to the archive tier for evicted entries.
   */
  getText(absPath: string): string | null
  /**
   * JS: `cache.getBinary(absPath)` — cached bytes of a binary asset, or
   * `null`. Reads fall through to the archive tier.
   */
  getBinary(absPath: string): Array<number> | null
  /**
   * JS: `cache.listFiles()` — absolute paths of everything cached
   * (memory + archive tiers).
   */
  listFiles(): Array<string>
  /**
   * JS: `cache.markdownFiles()` — absolute paths of cached markdown
   * files (either tier).
   */
  markdownFiles(): Array<string>
  /**
   * Accepted Markdown layouts discovered from the theme's `.html`
   * templates, sorted by name.
   *
   * JS: `cache.markdownLayouts()`.
   */
  markdownLayouts(): Array<string>
  /**
   * Absolute paths to the configured theme's layouts, assets, and data
   * directories.
   */
  themeDirs(): ThemeDirs
  /**
   * JS: `cache.assetFiles()` — absolute paths of cached non-markdown,
   * non-liquid files (either tier).
   */
  assetFiles(): Array<string>
  /**
   * JS: `cache.pages()` — every page object, for Liquid themes
   * (`{{ page.title }}`, `{{ page.layout }}`, …).
   */
  pages(): Array<PageObject>
  /**
   * JS: `cache.getPage(inputPath)` — a single page object by absolute
   * input path, or `null`.
   */
  getPage(inputPath: string): PageObject | null
  /** JS: `cache.assets()` — every asset object. */
  assets(): Array<AssetsObject>
  /**
   * JS: `cache.getAsset(inputPath)` — a single asset object by absolute
   * input path, or `null`.
   */
  getAsset(inputPath: string): AssetsObject | null
  /**
   * JS: `cache.liquids()` — every liquid object for the configured
   * `liquidCollections`: front-matter fields from the JS-side
   * `liquidRender(file)` callback (which also renders the HTML),
   * path-derived fields from the cache.
   */
  liquids(): Array<LiquidObject>
  /**
   * JS: `cache.getLiquid(inputPath)` — a single liquid object by
   * absolute input path, or `null`.
   */
  getLiquid(inputPath: string): LiquidObject | null
  /**
   * JS: `cache.liquidFiles()` — absolute paths of the tracked liquid
   * template files (either tier).
   */
  liquidFiles(): Array<string>
  /**
   * Absolute path to the configured theme package — or, in local
   * mode (no `theme` configured), the entry directory the local
   * theme directories resolve from.
   */
  themePath(): string
  /** Parsed theme metadata exposed as an object to Rust and JavaScript. */
  themeConfig(): BaganThemeConfig
  /**
   * JS: `cache.archived()` — absolute paths flushed to the disk
   * archive. Everything not listed still lives in memory only.
   */
  archived(): Array<string>
  /**
   * JS: `cache.mapJson()` — the current `map.json` document (path →
   * shard location + metadata, including front matter for search).
   */
  mapJson(): string
  /**
   * JS: `cache.flush()` — write all pending archive entries now,
   * blocking until the worker is done.
   */
  flush(): void
  /**
   * JS: `cache.baseUrl()` — the resolved base URL path prefix for the
   * configured hosting (e.g. `/repo/` for a GitHub Pages project site).
   */
  baseUrl(): string
  /**
   * JS: `cache.resolveUrl(path)` — prefix a site-relative path with the
   * resolved base URL: `resolveUrl("docs/intro")` → `/repo/docs/intro`.
   */
  resolveUrl(path: string): string
  /**
   * JS: `cache.siteUrl()` — absolute origin URL for canonical URLs, or
   * `null` when not configured.
   */
  siteUrl(): string | null
  /**
   * JS: `cache.onChange(cb)` — invoke `cb(event)` on every file change the
   * watcher detects while watching is active. Returns a subscription id
   * that can be passed to `offChange(id)`.
   *
   * The callback is wrapped in a `ThreadsafeFunction` because notify
   * events fire on the watcher thread, not the JS main thread.
   */
  onChange(callback: (event: CacheChangeEvent) => void): number
  /** JS: `cache.offChange(id)` — remove a previously registered callback. */
  offChange(id: number): void
  /**
   * JS: `cache.dispose()` — stop the watcher, flush pending archive
   * writes, and drop JS change callbacks. Safe to call more than once.
   *
   * Rust consumers do not need this: dropping the [`BaganCache`] stops
   * the watcher, flushes the archive, and joins the workers.
   */
  dispose(): void
}

/** Flush thresholds for the disk-archive tier. */
export interface ArchiveConfig {
  /**
   * Pending-content threshold (bytes) that triggers writing a new
   * shard; `0` disables size-based flushing (interval/drop only).
   */
  shardBytes?: number
  /**
   * Maximum time (ms) pending entries may sit in the worker buffer
   * before being flushed. `0` disables the timer.
   */
  flushIntervalMs?: number
}

/** An asset mirrored by the cache (JS/CSS/image/video/font/…). */
export interface AssetsObject {
  /** File name without extension (`foo.png` → `foo`). */
  fileName: string
  /** URL for the asset (`path/to/foo.png` → `/path/to/foo.png`). */
  slug: string
  /** Raw content: UTF-8 text for textual assets, base64 for binaries. */
  raw: string
  /** Build output path for the asset. */
  outFilePath: string
  /** Original absolute source path (watch-process identity). */
  inputFilePath: string
  /** File extension (lowercase, without dot). */
  ext: string
  /**
   * Kind classification: `"Js"`, `"Css"`, `"Image"`, `"Video"`,
   * `"Font"`, or `"Other"`.
   */
  fileType: string
}

/**
 * Cache-wide configuration: directories, markdown options, memory/archive
 * budgets, hosting/baseUrl resolution, and the build output directory.
 */
export interface BaganCacheConfig {
  /**
   * Entry directory holding the markdown files (absolute, or relative
   * to the current working directory). Defaults to the current directory.
   */
  entryDir?: string
  /**
   * Optional assets directory, resolved from the current working
   * directory and required to be inside `entry_dir`.
   */
  assetsDir?: string
  /**
   * Theme package name (Jekyll-gem style). When set, the theme is
   * resolved from `node_modules/<theme>` and its default export is
   * read as the theme configuration. When unset, the site is local:
   * the theme directories below are resolved from the entry
   * directory instead.
   */
  theme?: string
  /**
   * Local layouts directory (used when `theme` is unset), resolved
   * from the entry directory. Defaults to `_layouts`.
   */
  themeLayoutsDir?: string
  /**
   * Local includes directory (used when `theme` is unset), resolved
   * from the entry directory. Defaults to `_includes`.
   */
  themeIncludesDir?: string
  /**
   * Local assets directory (used when `theme` is unset), resolved
   * from the entry directory. Defaults to `_assets`.
   */
  themeAssetsDir?: string
  /**
   * Local data directory (used when `theme` is unset), resolved from
   * the entry directory. Defaults to `_data`.
   */
  themeDataDir?: string
  /**
   * Theme configuration object (`themeConfigObject`). In theme mode
   * the napi constructor loads it automatically from the theme
   * package's default export; pass it explicitly to skip the
   * automatic load (required for pure-Rust consumers, which have no
   * JS runtime to evaluate the export). Ignored when `theme` is unset.
   */
  themeConfig?: BaganThemeConfig
  /**
   * Markdown parsing/rendering options applied to every page render
   * (GFM/Kramdown toggles, `fragment`, title, …). Custom visitors and
   * parser hooks are registered separately via `usePlugin()`.
   */
  markdown?: PeisarOptions
  /**
   * Memory-tier budgets (LRU eviction). Defaults: 64 MiB bytes,
   * unlimited entries.
   */
  memory?: MemoryConfig
  /** Disk-archive tier thresholds. Defaults: 8 MiB shards, 2 s flush. */
  archive?: ArchiveConfig
  /** Static-hosting baseUrl resolution (GitHub Pages/Vercel/…). */
  hosting?: HostingConfig
  /**
   * Build output directory used for `outFilePath` fields (default
   * `".bagan"`).
   */
  outDir?: string
  /**
   * Archive directory, relative to the current working directory
   * (default `".peisar-cache"`).
   */
  cacheDir?: string
  /**
   * Liquid (`.html`) collections inside the entry directory: each
   * entry is a directory (flattened recursively) or a single file,
   * resolved against the entry directory. The discovered files are
   * cached as text, watched for changes, and exposed as
   * [`LiquidObject`](crate::LiquidObject) values. Entries outside the
   * entry directory are rejected at construction.
   */
  liquidCollections?: Array<string>
}

/**
 * JS-side options accepted by the `BaganCache` constructor (a flattened
 * subset of [`BaganCacheConfig`]; memory budgets only apply through the
 * `withConfig` factory).
 */
export interface BaganCacheJsOptions {
  /**
   * Theme package name (Jekyll-gem style), resolved from
   * `node_modules/<theme>`. Omit (null/undefined) for a local site:
   * the theme directories below are then resolved from the entry
   * directory.
   */
  theme?: string
  /**
   * Local layouts directory (local mode only), relative to the entry
   * directory. Defaults to `_layouts`.
   */
  themeLayoutsDir?: string
  /**
   * Local includes directory (local mode only), relative to the entry
   * directory. Defaults to `_includes`.
   */
  themeIncludesDir?: string
  /**
   * Local theme assets directory (local mode only), relative to the
   * entry directory. Defaults to `_assets`.
   */
  themeAssetsDir?: string
  /**
   * Local data directory (local mode only), relative to the entry
   * directory. Defaults to `_data`.
   */
  themeDataDir?: string
  /**
   * Theme configuration object (`themeConfigObject`). When omitted
   * (and `theme` is set) it is loaded automatically from the theme
   * package's default export.
   */
  themeConfig?: BaganThemeConfig
  /** Markdown parsing/rendering options (`fragment`, GFM, …). */
  markdown?: PeisarOptions
  /** Disk-archive thresholds (shard size / flush interval). */
  archive?: ArchiveConfig
  /** Static-hosting baseUrl resolution. */
  hosting?: HostingConfig
  /** Build output directory. */
  outDir?: string
  /** Disk-archive directory. */
  cacheDir?: string
  /**
   * Liquid (`.html`) collections inside the entry directory: each
   * entry is a directory (flattened recursively) or a single file,
   * resolved against the entry directory. The discovered files are
   * cached as text, watched, and exposed through `cache.liquids()`.
   */
  liquidCollections?: Array<string>
  /**
   * JavaScript function that renders a Liquid collection file to a
   * page (`liquidRender(file: string) => PageObject`). The
   * callback extracts the YAML front matter from the raw file,
   * renders the Liquid content to HTML, and returns the page fields
   * (the object is converted leniently — `html` is the meaningful
   * field, anything else falls back to the raw front matter); the
   * cache fills the path-derived fields (fileName, slug,
   * outFilePath, inputFilePath) and `raw`. Invoked on the JS main
   * thread when liquid objects are built; a callback that throws or
   * returns nothing leaves the front-matter fields empty and `html`
   * as the raw template. Registerable later with `useLiquidRender()`
   * as well.
   */
  liquidRender?: (file: string) => PageObject
}

/**
 * Theme configuration.
 *
 * In theme mode this is the `themeConfigObject` the theme package
 * exports as its default export (Jekyll-gem style: the package itself
 * is the source of truth, `bagan-theme.json` is no longer read). In
 * local mode (no `theme` configured) the values come from the cache
 * configuration instead.
 */
export interface BaganThemeConfig {
  /** Theme package name. */
  name?: string
  /** Theme package version. */
  version?: string
  /** Optional theme author. */
  author?: string
  /**
   * Relative path to the theme's HTML layout templates (default
   * `"_layouts"`), resolved against the theme root.
   */
  layoutsDir?: string
  /**
   * Relative path to the theme's include templates (default
   * `"_includes"`), resolved against the theme root.
   */
  includesDir?: string
  /**
   * Relative path to the theme's static assets (default `"_assets"`),
   * resolved against the theme root.
   */
  assetsDir?: string
  /**
   * Relative path to the theme's data files (default `"_data"`),
   * resolved against the theme root.
   */
  dataDir?: string
}

/**
 * Resolved hosting configuration: the single source of truth for every
 * URL the pipeline emits (page slugs, asset links, absolute URLs).
 */
export interface BaseDirInfo {
  /** Hosting provider in effect. */
  provider: BaganHostingProvider
  /** Repository name (GitHub Pages project sites): `/<repo>/` prefix. */
  repo?: string
  /**
   * Custom site URL (`https://example.com` or `https://user.github.io/repo`).
   * Used verbatim as the URL root for [`Self::site_url`].
   */
  siteUrl?: string
  /**
   * Extra path prefix under the host (e.g. `subpath` for
   * `example.com/subpath/`).
   */
  pathPrefix?: string
}

/**
 * A file-change event delivered to change callbacks
 * (see [`BaganCache::on_change`]).
 */
export interface CacheChangeEvent {
  /** Absolute path of the changed file. */
  path: string
  /** `"create"`, `"modify"`, `"remove"`, or `"other"`. */
  kind: string
  /** `true` when the file is under the markdown entry directory. */
  isMarkdown: boolean
}

/**
 * An entry stored in the cache: raw UTF-8 text (markdown and textual
 * assets) or raw bytes (binary assets). Exposed to JavaScript so JS
 * consumers can branch on the variant. The serde derives back the
 * bincode round-trip through the disk archive.
 */
export type CachedContent =
  | { type: 'Text'; field0: string }
  | { type: 'Binary'; field0: Array<number> }

/** Static-hosting configuration for baseUrl resolution. */
export interface HostingConfig {
  /** Hosting provider (see [`HostingProvider`]). */
  provider?: BaganHostingProvider
  /** GitHub repository name for project pages (`/<repo>/` prefix). */
  repo?: string
  /** Custom site URL (overrides provider defaults when set). */
  siteUrl?: string
  /** Extra path prefix (e.g. `"subpath"` for `example.com/subpath/`). */
  pathPrefix?: string
}

/**
 * A Liquid (`.html`) template tracked through one of the configured
 * `liquidCollections` directories/files. The template carries YAML front
 * matter like a markdown file; the JS-side `liquidRender(file)` callback
 * extracts the front matter, renders the Liquid content to HTML, and
 * returns the front-matter fields, so the object shares the
 * [`PageObject`] shape with markdown pages.
 *
 * Front-matter-derived fields (`layout`, `title`, `summary`, `tags`,
 * `publishDate`, `extra`) come from the callback's returned object
 * (falling back to the raw front matter when no callback is
 * configured); path-derived fields (`fileName`, `slug`, `outFilePath`,
 * `inputFilePath`) are computed by the cache.
 */
export interface LiquidObject {
  /**
   * Layout name from front matter (e.g. `default`, `page`, `post`);
   * `"default"` when front matter does not set one.
   */
  layout: string
  /** File name without extension (`foo.html` → `foo`). */
  fileName: string
  /**
   * Request URL for the rendered file (`docs/foo.html` → `/docs/foo`);
   * the `html` extension is stripped and the resolved baseUrl is
   * prefixed.
   */
  slug: string
  /** Raw file content (front matter + Liquid template, UTF-8 text). */
  raw: string
  /**
   * HTML rendered from the Liquid content by the JS-side
   * `liquidRender(file)` callback. Empty when no callback is
   * configured.
   */
  html: string
  /** Build output path (`path/to/foo.html` → `outDir/path/to/foo.html`). */
  outFilePath: string
  /** Original absolute source path (watch-process identity). */
  inputFilePath: string
  /** Page title from front matter. */
  title?: string
  /** Short description from front matter. */
  summary?: string
  /** Category tags from front matter. */
  tags?: Array<string>
  /** Publication date from front matter (string form `yyyy-mm-dd`). */
  publishDate?: string
  /**
   * Any extra front-matter keys not mapped to a named field, exposed
   * verbatim (as strings) so themes can render `{{ page.custom }}`.
   */
  extra?: Record<string, string>
}

/** LRU budgets for the memory tier. */
export interface MemoryConfig {
  /**
   * Maximum total bytes of raw content kept in memory (`0` =
   * unlimited). When the budget is exceeded, the least recently used
   * entries are evicted (they remain readable from the disk archive).
   */
  byteBudget?: number
  /** Maximum number of entries in memory (`0` = unlimited). */
  entryBudget?: number
}

/**
 * A page rendered for a Liquid theme: front-matter fields flattened next
 * to pipeline-derived fields (layout, slug, html, …).
 */
export interface PageObject {
  /**
   * Layout name from front matter (e.g. `default`, `page`, `post`);
   * `"default"` when front matter does not set one.
   */
  layout: string
  /**
   * File name without extension (`foo.md` → `foo`); `index` for
   * directory roots. Used as a search / lookup key.
   */
  fileName: string
  /** Request URL for the page (`path/to/foo.md` → `/path/to/foo`). */
  slug: string
  /** HTML rendered from the markdown by Peisar. */
  html: string
  /**
   * Build output path (`path/to/foo.md` → `outDir/path/to/foo/index.html`),
   * relative to the output root unless absolute.
   */
  outFilePath: string
  /** Original absolute source path (watch-process identity). */
  inputFilePath: string
  /** Page title (from front matter, or the first `#` heading). */
  title?: string
  /** Short description from front matter. */
  summary?: string
  /** Category tags from front matter. */
  tags?: Array<string>
  /** Publication date from front matter (string form `yyyy-mm-dd`). */
  publishDate?: string
  /**
   * Any extra front-matter keys not mapped to a named field, exposed
   * verbatim (as strings) so themes can render `{{ page.custom }}`.
   */
  extra?: Record<string, string>
}

/** Resolved theme directories, as absolute paths. */
export interface ThemeDirs {
  /** Directory containing the theme's HTML layout templates. */
  layoutsDir: string
  /** Directory containing the theme's static assets. */
  assetsDir: string
  /** Directory containing the theme's data files. */
  dataDir: string
  /** Directory containing the theme's HTML includes files. */
  includesDir: string
}

/**
 * Parses one Markdown document and exposes its AST, HTML, and front matter
 * to JavaScript.
 *
 * Construct this class with the Markdown source and an optional
 * [`PeisarOptions`] object. Properties are computed from the source document;
 * accessing `ast`, `html`, or `astJson` also applies registered visitors.
 */
export declare class Peisar {
  /**
   * Creates a parser for `rawMd`.
   *
   * When `options` is omitted, GFM and Kramdown parsing are enabled and
   * `html` returns a complete HTML document.
   */
  constructor(rawMd: string, options?: PeisarOptions | undefined | null)
  /**
   * Gets the parsed Markdown document as a JavaScript AST object.
   *
   * Registered visitors run before the AST is returned. The returned value
   * is a clone, so reading this property does not consume the parser state.
   */
  get ast(): Document
  /**
   * Registers a unified JavaScript plugin.
   *
   * The plugin can provide `visitBlock` / `visitInline` visitor callbacks
   * and `parseBlock` / `parseInline` parser hooks, all in one object.
   * Visitor callbacks receive their node as a one-item tuple: for example,
   * `visitBlock([block]) { return { recurse: true }; }`.
   */
  use(plugin: Plugin): void
  /**
   * Renders the parsed Markdown as HTML using the configured render options.
   *
   * Registered visitors run before rendering.
   */
  get html(): string
  /** Gets the deserialized YAML front matter, or `null` when none exists. */
  get frontmatter(): any | null
  /**
   * Gets the parsed Markdown AST serialized as JSON.
   *
   * Registered visitors run before serialization.
   */
  get astJson(): string
}

/**
 * JavaScript options for Markdown parsing and HTML rendering.
 *
 * Every property is optional.  Omitted parsing options enable GFM and
 * Kramdown; omitted rendering options produce a complete HTML document with
 * charset and viewport metadata.
 *
 * # Defaults
 *
 * | Property   | Default     |
 * |------------|-------------|
 * | `gfm`      | `true`      |
 * | `kramdown` | `true`      |
 * | `fragment` | `false` (full document) |
 * | `charset`  | `true`      |
 * | `viewport` | `true`      |
 * | `file_name`, `title`, `body_class`, `style` | `null` |
 */
export interface PeisarOptions {
  /**
   * Enable GitHub Flavored Markdown (tables, strikethrough, task lists,
   * autolinks).  Default: `true`.
   */
  gfm?: boolean
  /**
   * Enable Kramdown-style block attributes (`{:#id .class key="val"}`).
   * Default: `true`.
   */
  kramdown?: boolean
  /** Optional file name to attach to the parsed document. */
  fileName?: string
  /**
   * If `true`, emit only the body content (no `<!DOCTYPE>`, `<html>`,
   * `<head>`, or `<body>` wrapper).  If `false`, emit a full HTML
   * document.  Default: `true` (fragment).
   */
  fragment?: boolean
  /**
   * Include a `<meta charset="utf-8">` in the head (only relevant when
   * `fragment` is `false`).  Default: `true`.
   */
  charset?: boolean
  /**
   * Include a `<meta name="viewport" content="width=device-width,
   * initial-scale=1.0">` in the head (only relevant when `fragment` is
   * `false`).  Default: `true`.
   */
  viewport?: boolean
  /**
   * Optional `<title>` for the HTML head (only relevant when
   * `fragment` is `false`).  Default: `None`.
   */
  title?: string
  /**
   * Optional additional CSS classes to add to `<body>` (only relevant
   * when `fragment` is `false`).  Default: `None`.
   */
  bodyClass?: string
  /**
   * Optional inline CSS to inject in a `<style>` tag in the head.
   * Default: `None`.
   */
  style?: string
}

export declare function renderDocumentHtml(doc: Document, opts?: RenderOptions | undefined | null): string

/** Options that control how the AST is rendered to HTML. */
export interface RenderOptions {
  /**
   * If `true`, emit only the body content (no `<!DOCTYPE>`, `<html>`,
   * `<head>`, or `<body>` wrapper).  If `false`, emit a full HTML
   * document.  Default: `true` (fragment).
   */
  fragment: boolean
  /**
   * Include a `<meta charset="utf-8">` in the head (only relevant when
   * `fragment` is `false`).  Default: `true`.
   */
  charset: boolean
  /**
   * Include a `<meta name="viewport" content="width=device-width,
   * initial-scale=1.0">` in the head (only relevant when `fragment` is
   * `false`).  Default: `true`.
   */
  viewport: boolean
  /**
   * Optional `<title>` for the HTML head (only relevant when
   * `fragment` is `false`).  Default: `None`.
   */
  title?: string
  /**
   * Optional additional CSS classes to add to `<body>` (only relevant
   * when `fragment` is `false`).  Default: `None`.
   */
  bodyClass?: string
  /**
   * Optional inline CSS to inject in a `<style>` tag in the head.
   * Default: `None`.
   */
  style?: string
}

/**
 * Markdown source parsed into an AST, with optional YAML front matter and
 * registered plugins.
 */
export declare class PeisarAst {}

/**
 * Options that control how Markdown is parsed.
 *
 * # Defaults
 *
 * Both GFM and Kramdown extensions are enabled by default.  To parse
 * strict CommonMark only:
 */
export interface AstOptions {
  /**
   * Enable GitHub Flavored Markdown (tables, strikethrough, task lists,
   * autolinks).  Default: `true`.
   */
  gfm: boolean
  /**
   * Enable Kramdown-style block attributes (`{:#id .class key="val"}`).
   * Default: `true`.
   */
  kramdown: boolean
  /**
   * Optional file name to attach to the parsed
   * [`Document`](crate::Document).
   */
  fileName?: string
}

/** Parsed Kramdown block attributes (`{:#id .class key="val"}`). */
export interface Attributes {
  /** HTML `id` attribute. */
  id?: string
  /** CSS class list. */
  classes?: Array<string>
  /** Arbitrary key/value attributes. */
  attributes?: Array<[string, string]>
}

/** Block-level nodes. */
export type Block =
  | { type: 'Heading'; /** Heading level (1–6). */
    level: number; /** Inline content of the heading. */
    children: Array<Inline>; /** Source span. */
    pos: Span; /** Optional Kramdown attributes. */
  attrs?: Attributes }
  | { type: 'Paragraph'; /** Inline content of the paragraph. */
    children: Array<Inline>; /** Source span. */
    pos: Span; /** Optional Kramdown attributes. */
  attrs?: Attributes }
  | { type: 'CodeBlock'; /** Language hint from the info string (e.g. `rust` from ` ```rust `). */
    lang?: string; /** The raw code content. */
    code: string; /** Source span. */
    pos: Span; /** Optional Kramdown attributes. */
  attrs?: Attributes }
  | { type: 'BlockQuote'; /** Nested block content inside the quote. */
    children: Array<Block>; /** Source span. */
    pos: Span; /** Optional Kramdown attributes. */
  attrs?: Attributes }
  | { type: 'List'; /** `true` for ordered lists, `false` for unordered. */
    ordered: boolean; /** The list items. */
    items: Array<ListItem>; /** Source span. */
    pos: Span; /** Optional Kramdown attributes. */
  attrs?: Attributes }
  | { type: 'ThematicBreak'; /** Source span. */
  pos: Span }
  | { type: 'HtmlBlock'; /** The raw HTML content. */
    html: string; /** Source span. */
    pos: Span; /** Optional Kramdown attributes. */
  attrs?: Attributes }
  | { type: 'Table'; /** The table structure (header, rows, alignments). */
    table: Table; /** Source span. */
    pos: Span; /** Optional Kramdown attributes. */
  attrs?: Attributes }
  | { type: 'LinkReferenceDefinition'; /** Normalized label (lowercased, trimmed). */
    label: string; /** The destination URL. */
    url: string; /** Optional link title. */
    title?: string; /** Source span. */
  pos: Span }
  | { type: 'Comment'; /** The comment text (markers stripped). */
    value: string; /** Source span. */
  pos: Span }

/**
 * Synchronous block-visitor callback. Receives a `Block`, returns
 * `VisitControlJs` (or `undefined` for no changes).
 */
export type BlockCallback = (arg: [Block]) => VisitorControl | null

/**
 * Synchronous block parser hook. Receives a [`BlockParserContext`], returns
 * a [`BlockParseResultJs`] (or `undefined` to decline).
 */
export type BlockParseCallback = (arg: [BlockParserContext]) => BlockParseResult | null

/** Context passed to block parser hooks. */
export interface BlockParserContext {
  /** The current (unmodified) source line. */
  line: string
  /** 0-based index of the current line in the (sub-)document. */
  lineIndex: number
  /** Remaining lines starting at the current line (`lines[0]` is `line`). */
  lines: Array<string>
}

/**
 * Result returned from the JS `parseBlock` callback.
 *
 * All fields are optional; omitting `block` (or returning `undefined`)
 * declines the position so the next hook / built-in parser handles it.
 */
export interface BlockParseResult {
  /** The parsed block node. */
  block?: Block
  /** Number of source lines consumed (must be at least `1`). */
  consumed?: number
}

/**
 * A Markdown document — the root of the AST.
 *
 * Contains the top-level block children and all link reference definitions
 * collected from the source text.
 */
export interface Document {
  /** Always `"root"`. */
  nodeType: string
  /** Optional source file name. */
  fileName?: string
  /** Span of the whole document in the source text. */
  pos: Span
  /** Top-level block children. */
  children: Array<Block>
  /** All link reference definitions collected from the document. */
  linkReferences: Array<LinkReferenceDefinition>
}

/** Inline-level nodes. */
export type Inline =
  | { type: 'Text'; /** The text value. */
    value: string; /** Source span. */
  pos: Span }
  | { type: 'Emphasis'; /** Emphasis level (italic or bold). */
    level: PeisarEmphasisLevel; /** Nested inline content. */
    children: Array<Inline>; /** Source span. */
  pos: Span }
  | { type: 'Code'; /** The code text. */
    code: string; /** Source span. */
  pos: Span }
  | { type: 'HtmlInline'; /** The raw HTML string. */
    html: string; /** Source span. */
  pos: Span }
  | { type: 'Strikethrough'; /** Nested inline content. */
    children: Array<Inline>; /** Source span. */
  pos: Span }
  | { type: 'HardBreak'; /** Source span. */
  pos: Span }
  | { type: 'SoftBreak'; /** Source span. */
  pos: Span }
  | { type: 'Image'; /** Alternative text. */
    alt: string; /** Image URL. */
    url: string; /** Optional title. */
    title?: string; /** Source span. */
  pos: Span }
  | { type: 'Link'; /** Link text (inline children). */
    text: Array<Inline>; /** Destination URL. */
    url: string; /** Optional link title. */
    title?: string; /** `true` if this is a GFM autolink (bare URL). */
    autolink: boolean; /** Source span. */
  pos: Span }
  | { type: 'LinkReference'; /** Link text (inline children). */
    text: Array<Inline>; /** Normalized label used to look up the reference. */
    label: string; /** Resolved destination URL. */
    url: string; /** Optional resolved title. */
    title?: string; /** Source span. */
  pos: Span }

/**
 * Synchronous inline-visitor callback. Receives an `Inline`, returns
 * `InlineVisitControlJs` (or `undefined` for no changes).
 */
export type InlineCallback = (arg: [Inline]) => InlineVisitorControl | null

/**
 * Synchronous inline parser hook. Receives an [`InlineParserContext`],
 * returns an [`InlineParseResultJs`] (or `undefined` to decline).
 */
export type InlineParseCallback = (arg: [InlineParserContext]) => InlineParseResult | null

/**
 * Context passed to inline parser hooks.
 *
 * On the JavaScript side the fields are exposed as `rest` and `index`.
 */
export interface InlineParserContext {
  /** Remaining inline text starting at the current character position. */
  rest: string
  /** 0-based character index of the current position in the inline text. */
  index: number
}

/**
 * Result returned from the JS `parseInline` callback.
 *
 * All fields are optional; omitting `inline` (or returning `undefined`)
 * declines the position so the next hook / built-in parser handles it.
 */
export interface InlineParseResult {
  /** The parsed inline node. */
  inline?: Inline
  /** Number of characters consumed (`0` or missing declines). */
  consumed?: number
}

/**
 * JS-facing mirror of [`InlineVisitControl`].
 *
 * Returned from the JS `visitInline` callback.  All fields are optional;
 * omitting a field means "no change" for that operation.
 */
export interface InlineVisitorControl {
  /** Nodes to insert before the current node. */
  insertBefore?: Array<Inline>
  /** Nodes to insert after the current node. */
  insertAfter?: Array<Inline>
  /** Replace the current node with these nodes. */
  replaceWith?: Array<Inline>
  /** Remove the current node entirely. */
  remove?: boolean
  /** Whether to recurse into this node's children. */
  recurse?: boolean
}

/** A link reference definition collected at the document level. */
export interface LinkReferenceDefinition {
  /** Normalized label (lowercased, trimmed). */
  label: string
  /** The destination URL. */
  url: string
  /** Optional link title. */
  title?: string
  /** Source span. */
  pos: Span
}

/** A single list item (an `<li>`). Contains nested block content. */
export interface ListItem {
  /** Nested block content of the item. */
  children: Array<Block>
  /** GFM task-list state: `None` = not a task, `Some` = checked / unchecked. */
  task?: PeisarTaskState
  /** Span of the item in the *sub-document* of its enclosing list. */
  pos: Span
}

/**
 * A plugin containing optional Markdown parser hooks and AST visitor
 * callbacks.
 */
export interface Plugin {
  /** Optional JS callback for block nodes (JS: `visitBlock`). */
  visitBlock?: BlockCallback
  /** Optional JS callback for inline nodes (JS: `visitInline`). */
  visitInline?: InlineCallback
  /** Optional JS block parser hook (JS: `parseBlock`). */
  parseBlock?: BlockParseCallback
  /** Optional JS inline parser hook (JS: `parseInline`). */
  parseInline?: InlineParseCallback
}

/** A zero-based point in the source text. */
export interface Position {
  /** Line number, 0-based. */
  line: number
  /** Column number, 0-based (in characters). */
  column: number
  /** Byte offset from the start of the input. */
  offset: number
}

/**
 * A half-open span `[start, end)` covering a node's source text.
 *
 * `start` is inclusive and `end` is exclusive; both positions point into the
 * original source string.
 */
export interface Span {
  /** The start position (inclusive). */
  start: Position
  /** The end position (exclusive). */
  end: Position
}

/** A GFM table. */
export interface Table {
  /** The header row. */
  header: TableRow
  /** Body rows. */
  rows: Array<TableRow>
  /** Column alignment specifications (one per column). */
  alignments: PeisarTableCellAlignments
}

/** A single table cell. */
export interface TableCell {
  /** Inline content of the cell. */
  children: Array<Inline>
}

/** A single table row (header or body). */
export interface TableRow {
  /** The cells in this row. */
  cells: Array<TableCell>
}

/**
 * JS-facing mirror of [`VisitControl`].
 *
 * Returned from the JS `visitBlock` callback.  All fields are optional;
 * omitting a field means "no change" for that operation.
 */
export interface VisitorControl {
  /** Nodes to insert before the current node. */
  insertBefore?: Array<Block>
  /** Nodes to insert after the current node. */
  insertAfter?: Array<Block>
  /** Replace the current node with these nodes. */
  replaceWith?: Array<Block>
  /** Remove the current node entirely. */
  remove?: boolean
  /** Whether to recurse into this node's children. */
  recurse?: boolean
}

/**
 * Parses YAML front matter from Markdown content.
 *
 * Returns an N-API error if the front matter is present but invalid.
 */
export declare function frontmatter(content: string): FrontmatterResult

export interface FrontmatterResult {
  /** Clean content after the leading YAML front matter is removed. */
  cleanContent: string
  /** Deserialized YAML front matter, or `null` when no front matter exists. */
  yamlData?: Record<string, any>
}

/**
 * Reads and parses a YAML file relative to the process's current working
 * directory.
 *
 * The parsed YAML value is returned to Node.js as a JavaScript value. File
 * read failures panic in this N-API wrapper; parse failures are returned as
 * N-API errors.
 */
export declare function loadYaml(filePath: string): Record<string, any>

/**
 * Parses a YAML document into a JSON-compatible value.
 *
 * When consumed from Node.js, the result is exposed as a JavaScript object
 * with the TypeScript type `Record<string, any>`.
 *
 * Returns an N-API error if the YAML input cannot be parsed.
 */
export declare function yamlParser(yamlStr: string): Record<string, any>
