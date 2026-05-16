#!/usr/bin/env bash
# Assemble a self-contained Typst package under dist/typstuml/<version>/.
#
# Contents of the assembled directory:
#   typst.toml         metadata
#   lib.typ            Typst-side API (plugin loader, eval scope, render-puml)
#   typstuml.wasm      compiled plugin from ./build.sh
#   blockcell/         vendored slim blockcell (lib.typ + src/*.typ)
#   LICENSE
#   README.md
#
# The blockcell vendoring mirrors what build.rs stages for the CLI build
# (`STAGED_LIB_TYP` + `STAGED_SRC_FILES`). Keep the lists in this script
# in sync with build.rs — drifting them is what golden tests will catch.
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
BC_SRC=../../vendor/blockcell/src
BC_DEST=$DEST/blockcell

# Mirror of `STAGED_SRC_FILES` in /Users/dalei/github/TypstUML/build.rs.
# If you add a new diagram type that pulls in a different blockcell file,
# add it here AND there.
BLOCKCELL_FILES=(
  records.typ
  seq-puml.typ
  seq.typ
  tree.typ
  cuca.typ
  cuca/theme.typ
  cuca/shape-card.typ
  cuca/shape-desc.typ
  cuca/edges.typ
  states.typ
  atoms.typ
  composites.typ
  containers.typ
  flows.typ
  palettes.typ
  internal/metrics.typ
  internal/stroke.typ
)

# Mirror of `STAGED_LIB_TYP` in /Users/dalei/github/TypstUML/build.rs.
BLOCKCELL_LIB_TYP='// Slim re-export for TypstUML. See build.rs for the full rationale.
#import "src/records.typ": record-layout, record-probe
#import "src/seq-puml.typ": seq-puml
#import "src/tree.typ": tree, node, mindmap
#import "src/cuca.typ": cuca-layout, cuca-probe, container-probe
#import "src/states.typ": state-layout, state-probe, state-note-probe
#import "src/atoms.typ": process, decision, terminal, junction, edge, flow-node
#import "src/composites.typ": flow-col, section
#import "src/flows.typ": branch, branch-merge, switch, case, n-way, fork-bar, flow-loop, start-marker, stop-marker, end-marker, detach-marker, partition, flow-note, with-notes, swimlane, lane
'

if [[ ! -d $BC_SRC ]]; then
  echo "error: $BC_SRC missing — run 'git submodule update --init vendor/blockcell'" >&2
  exit 1
fi

rm -rf "$DEST"
mkdir -p "$DEST" "$BC_DEST/src"

# 1. Package-level files
cp package/typst.toml    "$DEST/typst.toml"
cp package/lib.typ       "$DEST/lib.typ"
cp package/README.md     "$DEST/README.md"
cp ../../LICENSE         "$DEST/LICENSE"
cp pkg/typstuml.wasm     "$DEST/typstuml.wasm"

# 2. Slim blockcell vendor
printf '%s' "$BLOCKCELL_LIB_TYP" > "$BC_DEST/lib.typ"
for rel in "${BLOCKCELL_FILES[@]}"; do
  src="$BC_SRC/$rel"
  dst="$BC_DEST/src/$rel"
  mkdir -p "$(dirname "$dst")"
  if [[ ! -f $src ]]; then
    echo "error: blockcell file $src missing — submodule may be out of date" >&2
    exit 1
  fi
  cp "$src" "$dst"
done

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
