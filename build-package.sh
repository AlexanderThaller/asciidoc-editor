#!/usr/bin/env bash
# Builds the editor as a package a web page can load.
#
# Produces pkg/asciidoc_editor.js (an ES module), its wasm, and TypeScript
# types. Nothing else needs serving: the stylesheets are inside the wasm.
set -euo pipefail

cd "$(dirname "$0")"

# The generator must match the wasm-bindgen the crate was built against, so
# take the version from the lockfile rather than whatever is on PATH. Trunk
# keeps the matching binaries it downloads.
version=$(awk '/^name = "wasm-bindgen"$/ { found = 1; next } found && /^version/ { gsub(/[",]/, "", $3); print $3; exit }' Cargo.lock)
generator="$HOME/.cache/trunk/wasm-bindgen-$version/wasm-bindgen"

if [ ! -x "$generator" ]; then
  if wasm-bindgen --version 2>/dev/null | grep -q " $version\$"; then
    generator=wasm-bindgen
  else
    echo "need wasm-bindgen $version: cargo install -f wasm-bindgen-cli --version $version" >&2
    exit 1
  fi
fi

cargo build --release --lib --target wasm32-unknown-unknown
"$generator" \
  --target web \
  --out-dir pkg \
  --out-name asciidoc_editor \
  target/wasm32-unknown-unknown/release/asciidoc_editor.wasm

# Shrinking matters here: the page pays for every byte on first load.
optimiser=$(find "$HOME/.cache/trunk" -name wasm-opt -type f 2>/dev/null | head -1)
if [ -z "$optimiser" ] && command -v wasm-opt >/dev/null; then
  optimiser=wasm-opt
fi

if [ -n "$optimiser" ]; then
  "$optimiser" -Oz --strip-debug \
    pkg/asciidoc_editor_bg.wasm -o pkg/asciidoc_editor_bg.wasm
else
  echo "wasm-opt not found: shipping the unoptimised wasm" >&2
fi

# ---- npm metadata ------------------------------------------------------
#
# Written here rather than kept in pkg/, which is generated: the version and
# description have one home, in Cargo.toml, and cannot drift from it.

field() {
  awk -v key="$1" '
    /^\[/ { in_package = ($0 == "[package]"); next }
    in_package && $1 == key {
      sub(/^[^=]*=[[:space:]]*/, "")
      gsub(/^"|"$/, "")
      print
      exit
    }
  ' Cargo.toml
}

cat > pkg/package.json <<JSON
{
  "name": "$(field name)",
  "version": "$(field version)",
  "description": "$(field description)",
  "license": "$(field license)",
  "type": "module",
  "main": "asciidoc_editor.js",
  "types": "asciidoc_editor.d.ts",
  "exports": {
    ".": {
      "types": "./asciidoc_editor.d.ts",
      "default": "./asciidoc_editor.js"
    },
    "./asciidoc_editor_bg.wasm": "./asciidoc_editor_bg.wasm",
    "./package.json": "./package.json"
  },
  "sideEffects": false,
  "files": [
    "asciidoc_editor.js",
    "asciidoc_editor.d.ts",
    "asciidoc_editor_bg.wasm",
    "asciidoc_editor_bg.wasm.d.ts",
    "README.md",
    "LICENSE"
  ],
  "keywords": [
    "asciidoc",
    "editor",
    "wysiwyg",
    "rich-text",
    "wasm",
    "webassembly",
    "rust"
  ]
}
JSON

# npm shows the README on the package page, and the licence must travel with
# the code it covers.
cp README.md LICENSE pkg/

printf '\npkg/ holds:\n'
ls -lh pkg | awk 'NR > 1 { printf "  %-30s %s\n", $9, $5 }'
printf '\nto publish: npm publish pkg\n'
