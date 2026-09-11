#!/usr/bin/env bash
# Builds the editor as a package a web page can load.
#
# Produces pkg/: an ES module, its wasm, TypeScript types, and the npm
# metadata. Nothing else needs serving — the stylesheets are inside the wasm.
#
# The tools come from the flake: `nix develop --command ./build-package.sh`,
# or `nix run .#package`. Outside Nix anything on PATH will do, so long as
# wasm-bindgen agrees with the lockfile.
set -euo pipefail

cd "$(dirname "$0")"

# The generator writes the glue for a particular wasm-bindgen ABI, so it has to
# be the same version the crate was compiled against. A mismatch produces a
# module the browser rejects at load, which is a miserable thing to debug.
version=$(awk '/^name = "wasm-bindgen"$/ { found = 1; next }
               found && /^version/ { gsub(/[",]/, "", $3); print $3; exit }' Cargo.lock)

generator=$(command -v wasm-bindgen || true)
if [ -z "$generator" ] || ! "$generator" --version | grep -q " $version\$"; then
  # Trunk keeps matching binaries it has downloaded; use one if it is there.
  cached="$HOME/.cache/trunk/wasm-bindgen-$version/wasm-bindgen"
  if [ -x "$cached" ]; then
    generator="$cached"
  else
    echo "need wasm-bindgen $version; 'nix develop' provides it" >&2
    exit 1
  fi
fi

optimiser=$(command -v wasm-opt || true)
if [ -z "$optimiser" ] && [ -d "$HOME/.cache/trunk" ]; then
  optimiser=$(find "$HOME/.cache/trunk" -name wasm-opt -type f | head -1 || true)
fi

if [ -z "$optimiser" ] && [ "${ALLOW_UNOPTIMISED:-}" != "1" ]; then
  # Unoptimised the wasm is about three times the size. Publishing that by
  # accident is worse than stopping here, so it has to be asked for.
  echo "need wasm-opt; 'nix develop' provides it, or set ALLOW_UNOPTIMISED=1" >&2
  exit 1
fi

cargo build --release --lib --target wasm32-unknown-unknown

"$generator" \
  --target web \
  --out-dir pkg \
  --out-name asciidoc_editor \
  target/wasm32-unknown-unknown/release/asciidoc_editor.wasm

# Shrinking matters here: the page pays for every byte on first load.
# --converge repeats until nothing more comes off, and the strips drop
# sections a browser never reads. Written aside and moved, so a failure
# cannot leave a half-written module behind.
if [ -n "$optimiser" ]; then
  "$optimiser" -Oz --converge --strip-debug --strip-producers \
    --strip-target-features \
    pkg/asciidoc_editor_bg.wasm -o pkg/asciidoc_editor_bg.wasm.opt
  mv pkg/asciidoc_editor_bg.wasm.opt pkg/asciidoc_editor_bg.wasm
else
  echo "no wasm-opt: shipping the unoptimised wasm" >&2
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
  "repository": {
    "type": "git",
    "url": "git+$(field repository).git"
  },
  "homepage": "$(field repository)#readme",
  "bugs": {
    "url": "$(field repository)/issues"
  },
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
printf '\nto publish: npm publish ./pkg\n'
