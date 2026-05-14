#!/usr/bin/env bash
# Build the size-optimized wasm package into ./pkg/.
#
# This drives `cargo` and `wasm-bindgen` directly instead of going through
# `wasm-pack build`. wasm-pack only wires its wasm-opt step to the built-in
# `dev`/`release`/`profiling` profiles — it ignores the custom `wasm-release`
# cargo profile we use (so native `cargo build --release` stays untouched) and
# can't be told to pass wasm-opt the `--all-features` flag that rustc's
# post-MVP wasm output requires. Doing the three stages by hand sidesteps both.
#
# Stages:
#   1. cargo build --profile wasm-release   — opt-level=z, fat LTO, strip, panic=abort
#   2. wasm-bindgen --target web            — JS glue + the _bg.wasm module
#   3. wasm-opt -Oz --all-features          — binaryen size pass
#
# Usage:  ./build.sh           # all three stages
#         ./build.sh --no-opt  # skip stage 3 (no wasm-opt needed)
set -euo pipefail
cd "$(dirname "$0")"

PROFILE=wasm-release
TARGET=wasm32-unknown-unknown
NAME=typstuml_wasm
WASM=pkg/${NAME}_bg.wasm

# Locate a tool on PATH, falling back to the copy wasm-pack downloads into its
# own cache (handy on a machine that has run wasm-pack at least once).
find_tool() {
  local found
  found="$(command -v "$1" || true)"
  if [[ -z "$found" ]]; then
    found="$(find "$HOME/Library/Caches/.wasm-pack" "$HOME/.cache/.wasm-pack" \
      -name "$1" -type f 2>/dev/null | head -1 || true)"
  fi
  printf '%s' "$found"
}

echo ">> cargo build --profile $PROFILE --target $TARGET"
cargo build -p typstuml-wasm --profile "$PROFILE" --target "$TARGET"

WASM_BINDGEN="$(find_tool wasm-bindgen)"
if [[ -z "$WASM_BINDGEN" ]]; then
  echo "!! wasm-bindgen CLI not found. Install the version that matches the"
  echo "   wasm-bindgen crate in Cargo.lock:  cargo install wasm-bindgen-cli"
  exit 1
fi
echo ">> $WASM_BINDGEN --target web --out-dir pkg"
"$WASM_BINDGEN" --target web --out-dir pkg --out-name "$NAME" \
  "../../target/$TARGET/$PROFILE/$NAME.wasm"

if [[ "${1:-}" == "--no-opt" ]]; then
  echo ">> skipping wasm-opt (--no-opt)"
else
  WASM_OPT="$(find_tool wasm-opt)"
  if [[ -z "$WASM_OPT" ]]; then
    echo ">> wasm-opt not found — install binaryen to shrink the module further:"
    echo "   brew install binaryen   (or)   cargo install wasm-opt"
  else
    echo ">> $WASM_OPT -Oz --all-features"
    "$WASM_OPT" -Oz --all-features "$WASM" -o "$WASM.tmp"
    mv "$WASM.tmp" "$WASM"
  fi
fi

size=$(wc -c < "$WASM")
gz=$(gzip -c "$WASM" | wc -c)
printf '>> %s: %.1f MB raw, %.1f MB gzipped\n' "$WASM" \
  "$(echo "$size / 1048576" | bc -l)" "$(echo "$gz / 1048576" | bc -l)"
