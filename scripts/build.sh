#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

MJS_FILE="index.mjs"
BINARY_FILE="bagan_cache.linux-x64-gnu.node"
DTS_FILE="types.d.ts"

cat << 'EOF' > "$DTS_FILE"
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
EOF

sleep 3



# 2. Build to build dir
npx napi build --platform --js index.cjs --config-path napi.config.json



cat << 'EOF' > "$MJS_FILE"
// Generated form ci.yml
import { createRequire } from "module";
const require = createRequire(import.meta.url);
const bagan_cache = require("./index.cjs");

export const BaganCache = bagan_cache.BaganCache;
export const Peisar = bagan_cache.Peisar;
export const renderDocumentHtml = bagan_cache.renderDocumentHtml;
export const PeisarAst = bagan_cache.PeisarAst;
export const frontmatter = bagan_cache.frontmatter;
export const loadYaml = bagan_cache.loadYaml;
export const yamlParser = bagan_cache.yamlParser;
EOF

# 3.1 Lint and format output JS files

# 3.1.1 Lint JS files

npx oxlint index.cjs
npx oxlint index.mjs

# 3.1.2 Format JS files

npx oxfmt --write index.cjs
npx oxfmt --write index.mjs
npx oxfmt --write index.d.ts

# 4. Remove binary and .d.ts
rm -rf "$BINARY_FILE"
sleep 3
rm -rf "$DTS_FILE"


# 5. Write gitHead

node -e "
const fs = require('fs');
const pkg = require('./package.json');
try {
  pkg.gitHead = require('child_process').execSync('git rev-parse HEAD').toString().trim();
  fs.writeFileSync('./package.json', JSON.stringify(pkg, null, 2));
  console.log('Successfully added gitHead:', pkg.gitHead);
} catch (e) {
  console.error('Failed to read git HEAD or write package.json', e);
}
"

# Format package.json after write gitHead

npx oxfmt --write package.json

echo "Finished Build"