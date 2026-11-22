#!/bin/sh
# Language servers that ship inside the app and run on Node.js. pyright is
# self-contained; bash-language-server needs its npm dependencies, so it gets
# its own install folder for the app bundle (see tauri.conf.json resources).
set -eu
cd "$(dirname "$0")/.."
version=$(node -p "require('./package.json').dependencies['bash-language-server']")
dest=src-tauri/resources/bash-language-server
rm -rf "$dest"
mkdir -p "$dest"
npm install --prefix "$dest" --no-save --omit=dev --no-audit --no-fund --no-package-lock "bash-language-server@$version" >/dev/null
echo "bundled bash-language-server $version"
