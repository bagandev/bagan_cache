#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

DES_DIR="local/cache"
DTS_FILE="types.d.ts"
PACKAGE_FILE="$DES_DIR/package.json"

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



# Build to build dir
npx napi build --platform  --config-path napi.config.json --output-dir "$DES_DIR"

sleep 3

cat << 'EOF' > "$PACKAGE_FILE"
{
  "name": "cache",
  "main": "index.js",
  "types": "index.d.ts",
  "type": "commonjs"
}
EOF

rm -rf "$DTS_FILE"

echo "Finished Build"