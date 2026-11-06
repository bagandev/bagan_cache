# Bagan Cache

Memory-first cache of a site's Markdown and asset files, exposed to Node
through napi-rs. Themes load the Jekyll way:

## Theme resolution

- **`theme` unset (local mode)** — the theme directories are resolved
  from the current working directory: `_layouts`, `_includes`, `_assets`,
  and `_data`. Each can be overridden in the cache options
  (`themeLayoutsDir`, `themeIncludesDir`, `themeAssetsDir`,
  `themeDataDir`).

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
