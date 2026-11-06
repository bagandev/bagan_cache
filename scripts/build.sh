#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

BINARY_FILE="bagan_cache.linux-x64-gnu.node"

sleep 3

# 2. Build to build dir
npx napi build --platform --features npm  --config-path napi.config.json


# 3.1 Lint and format output JS files

# 3.1.1 Lint JS files

npx oxlint index.js

# 3.1.2 Format JS files

npx oxfmt --write index.js
npx oxfmt --write index.d.ts

# 4. Remove binary and .d.ts
rm -rf "$BINARY_FILE"


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