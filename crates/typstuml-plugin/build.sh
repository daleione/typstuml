#!/usr/bin/env bash
# Build the Typst plugin wasm artifact into ./pkg/typstuml.wasm.
#
# Stages:
#   1. cargo build --profile wasm-release   — opt-level=z, fat LTO, strip, panic=abort
#                                              (defined at the workspace root Cargo.toml)
#   2. wasm-opt -Oz --all-features          — optional, requires binaryen on PATH
#
# This is a typstuml-plugin-specific script — unlike the playground at
# crates/typstuml-wasm/, the plugin doesn't need wasm-bindgen. Typst loads
# the wasm via #plugin(...) using the wasm-minimal-protocol ABI (the host
# imports come from Typst itself, no JS shim required).
#
# Usage:  ./build.sh           # build + wasm-opt
#         ./build.sh --no-opt  # skip wasm-opt
set -euo pipefail
cd "$(dirname "$0")"

PROFILE=wasm-release
TARGET=wasm32-unknown-unknown
CRATE_OUT=../../target/$TARGET/$PROFILE/typstuml_plugin.wasm
OUT=pkg/typstuml.wasm

mkdir -p pkg

echo ">> cargo build --profile $PROFILE --target $TARGET"
cargo build -p typstuml-plugin --profile "$PROFILE" --target "$TARGET"

# Plugin Cargo.toml sets default-features=false on typstuml, so the
# `embed-typst` feature is off — the wasm artifact only contains
# parser + IR + layout + codegen, not the typst-as-library stack.
cp "$CRATE_OUT" "$OUT"

run_wasm_opt() {
  local in="$1"
  echo ">> wasm-opt -Oz --all-features $in"
  wasm-opt -Oz --all-features "$in" -o "$in.tmp"
  mv "$in.tmp" "$in"
}

if [[ "${1:-}" == "--no-opt" ]]; then
  echo ">> skipping wasm-opt (--no-opt)"
elif command -v wasm-opt >/dev/null 2>&1; then
  run_wasm_opt "$OUT"
else
  echo ">> wasm-opt not found — install binaryen for a smaller artifact:" >&2
  echo "   brew install binaryen   (or)   cargo install wasm-opt" >&2
fi

size=$(wc -c < "$OUT")
gz=$(gzip -c "$OUT" | wc -c)
printf '>> %s: %.1f KB raw, %.1f KB gzipped\n' "$OUT" \
  "$(echo "$size / 1024" | bc -l)" "$(echo "$gz / 1024" | bc -l)"
