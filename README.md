# Bagan Cache

Memory-first cache of a site's Markdown and asset files, exposed to Node
through napi-rs. Themes load the Jekyll way:

## Theme resolution

- **`theme` unset (local mode)** — the theme directories are resolved
  from the entry directory: `_layouts`, `_includes`, `_assets`, and
  `_data`. Each can be overridden in the cache options
  (`themeLayoutsDir`, `themeIncludesDir`, `themeAssetsDir`,
  `themeDataDir`), relative to the entry directory.

- **`theme` set (theme mode)** — the theme is resolved from
  `node_modules/<theme>` and its default export is read as the
  `themeConfigObject`:

  ```js
  // node_modules/my-bagan-theme/index.js
  module.exports = {
    default: {
      name: "my-bagan-theme",
      version: "1.0.0",
      layoutsDir: "_layouts", // optional, defaults shown
      includesDir: "_includes",
      assetsDir: "_assets",
      dataDir: "_data",
    },
  };
  ```

  The old `bagan-theme.json` file is no longer read. Pure asset themes
  (layouts/templates only, no JS entry) fall back to their `package.json`
  metadata for the theme config; Rust consumers (no JS runtime) can pass
  `themeConfig` explicitly instead.

## JS usage

```js
const { BaganCache } = require("@bagandev/bagan_cache");

// Local mode:
const cache = new BaganCache("contents", null, {
  markdown: { fragment: true },
});

// Theme mode (default export auto-loaded):
const themed = new BaganCache("contents", null, { theme: "bagan-minima" });

cache.pages(); // PageObject[] — for Liquid themes
cache.themeDirs(); // absolute paths of the theme directories
cache.dispose();
```

## Liquid collections

Local Liquid (`.html`/`.liquid`) pages are opted-in through
`liquidCollections` — directories (flattened recursively) or single
files, resolved against the entry directory and required to live inside
it:

```js
const cache = new BaganCache("contents", null, {
  liquidCollections: ["_pages", "about.html"],
  // Renders a template to HTML on the JS side (optional; also
  // registerable later with `cache.useLiquidRender(cb)`):
  liquidRender: (file) => liquid.parseAndRenderSync(file),
});

cache.liquids(); // LiquidObject[]: { fileName, slug, raw, html,
//   outFilePath, inputFilePath, ext }
cache.getLiquid(absPath); // single LiquidObject (or null)
cache.liquidFiles(); // absolute paths of the tracked files
```

Tracked files are cached like pages, watched for changes (create/modify/
remove keep `liquids()` in sync), and excluded from `assets()`/`assetFiles()`.
Slugs are relative to the collection directory for directory collections
(`_pages/about.html` → `/about`) and to the entry directory for single
files; `about.liquid.html` keeps its emitted `.html` extension
(`/about.html`).

## Development

Run the filesystem watcher from the project root with:

```sh
cargo run --example cache -- [entry-directory]
```

The entry directory defaults to the current directory and must contain a
supported `index` Markdown file. Press Enter to stop watching and flush
pending cache writes.

Build the npm artifacts with:

```sh
npm run build
```
