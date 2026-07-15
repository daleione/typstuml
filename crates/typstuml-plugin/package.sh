#!/usr/bin/env bash
# Assemble a self-contained Typst package under dist/typstuml/<version>/.
#
# Contents of the assembled directory:
#   typst.toml         metadata
#   lib.typ            Typst-side API (plugin loader, eval scope, render-puml)
#   typstuml.wasm      compiled plugin from ./build.sh
#   blockcell/         copy of ../../components/ (lib.typ + src/*.typ)
#   LICENSE
#   README.md
#
# Usage:
#   ./package.sh                  # assemble dist/typstuml/<version>/
#   ./package.sh --install-local  # also copy to @local
#   ./package.sh --publish-prep   # sanity-check the assembled dir
set -euo pipefail
cd "$(dirname "$0")"

if [[ ! -f pkg/typstuml.wasm ]]; then
  echo "error: pkg/typstuml.wasm is missing — run ./build.sh first" >&2
  exit 1
fi

VERSION=$(grep '^version' package/typst.toml | head -1 | cut -d'"' -f2)
DEST=dist/typstuml/$VERSION
BC_SRC=../../components
BC_DEST=$DEST/blockcell

if [[ ! -d $BC_SRC ]]; then
  echo "error: $BC_SRC missing" >&2
  exit 1
fi

rm -rf "$DEST"
mkdir -p "$DEST"

# 1. Package-level files
cp package/typst.toml    "$DEST/typst.toml"
cp package/lib.typ       "$DEST/lib.typ"
cp package/README.md     "$DEST/README.md"
cp ../../LICENSE         "$DEST/LICENSE"
cp pkg/typstuml.wasm     "$DEST/typstuml.wasm"

# 2. Slim blockcell vendor (mirrors components/, the single source of truth)
cp -R "$BC_SRC" "$BC_DEST"

echo ">> assembled $DEST"
du -sh "$DEST"

case "${1:-}" in
  --install-local)
    # Typst's package cache is platform-specific:
    #   macOS : ~/Library/Application Support/typst/packages/
    #   Linux : $XDG_DATA_HOME/typst/packages/  (~/.local/share fallback)
    #   Win   : %APPDATA%\typst\packages\        (not handled here)
    if [[ "$(uname -s)" == "Darwin" ]]; then
      LOCAL_BASE="$HOME/Library/Application Support/typst/packages/local/typstuml"
    else
      LOCAL_BASE="${XDG_DATA_HOME:-$HOME/.local/share}/typst/packages/local/typstuml"
    fi
    LOCAL="$LOCAL_BASE/$VERSION"
    mkdir -p "$LOCAL_BASE"
    rm -rf "$LOCAL"
    cp -R "$DEST" "$LOCAL"
    echo ">> installed to $LOCAL"
    echo ">> use it as:  #import \"@local/typstuml:$VERSION\": render-puml"
    ;;
  --publish-prep)
    # Sanity: every file the package promises is present and non-empty.
    for f in typst.toml lib.typ typstuml.wasm LICENSE README.md blockcell/lib.typ; do
      [[ -s $DEST/$f ]] || { echo "missing or empty: $DEST/$f" >&2; exit 1; }
    done
    echo ">> looks publishable. Next: smoke-test, then PR to typst/packages."
    ;;
  "" ) ;;  # no extra step
  * )
    echo "unknown flag: $1 (try --install-local or --publish-prep)" >&2
    exit 1
    ;;
esac
