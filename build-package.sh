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
# Trunk keeps a wasm-opt in its cache, which is the one to prefer here. A
# machine that has never run trunk has no such directory — `find` then fails,
# and under `set -o pipefail` that would end the build with no explanation.
optimiser=""
if [ -d "$HOME/.cache/trunk" ]; then
  optimiser=$(find "$HOME/.cache/trunk" -name wasm-opt -type f | head -1 || true)
fi
if [ -z "$optimiser" ] && command -v wasm-opt >/dev/null; then
  optimiser=wasm-opt
fi

if [ -n "$optimiser" ]; then
  # --converge repeats until nothing more comes off; the strips drop sections
  # a browser never reads. Older binaryens do not know every one of these, and
  # a release is not worth failing over a flag, so fall back to the basics.
  if ! "$optimiser" -Oz --converge --strip-debug --strip-producers \
      --strip-target-features \
      pkg/asciidoc_editor_bg.wasm -o pkg/asciidoc_editor_bg.wasm 2>/dev/null; then
    echo "wasm-opt rejected the full flag set; optimising with -Oz alone" >&2
    "$optimiser" -Oz --strip-debug \
      pkg/asciidoc_editor_bg.wasm -o pkg/asciidoc_editor_bg.wasm
  fi
elif [ "${ALLOW_UNOPTIMISED:-}" = "1" ]; then
  echo "wasm-opt not found: shipping the unoptimised wasm" >&2
else
  # Without it the wasm is around three times the size. Publishing that by
  # accident is worse than failing here, so this has to be asked for.
  echo "wasm-opt not found; install binaryen, or set ALLOW_UNOPTIMISED=1" >&2
  exit 1
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
# The path must be a path: `npm publish pkg` reads `pkg` as the name of a
# package on the registry and tries to publish that one instead.
printf '\nto publish: npm publish ./pkg\n'
