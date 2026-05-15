#!/usr/bin/env bash
# Produce two distributables in dist/:
#
#   1. typstuml-playground.zip
#      Multi-file bundle (index.html + pkg/). Recipient unzips and serves
#      the directory over HTTP — needed because ES modules and WebAssembly
#      can't load over file://. About 8.5 MB compressed.
#
#   2. typstuml-playground.html
#      Single self-contained file. The no-modules wasm-bindgen shim and the
#      wasm bytes (base64) are inlined, so it runs by double-click off a
#      local disk. About 28 MB (base64 inflates the 20 MB wasm by ~33%).
#
# Both are independent — you can hand off whichever fits the audience.
# Run `./build.sh` first to populate `pkg/` and `pkg-nomod/`.

set -euo pipefail
cd "$(dirname "$0")"

if [[ ! -f pkg/typstuml_wasm_bg.wasm ]]; then
  echo "error: pkg/ is empty — run ./build.sh first" >&2
  exit 1
fi
if [[ ! -f pkg-nomod/typstuml_wasm_bg.wasm ]]; then
  echo "error: pkg-nomod/ is empty — run ./build.sh first (it now builds both targets)" >&2
  exit 1
fi

mkdir -p dist

# ---------------------------------------------------------------------------
# 1) typstuml-playground.zip — multi-file, HTTP-served
# ---------------------------------------------------------------------------

STAGE_NAME="typstuml-playground"
STAGE_DIR="$(mktemp -d)/$STAGE_NAME"
mkdir -p "$STAGE_DIR/pkg"

cp index.html              "$STAGE_DIR/"
cp pkg/typstuml_wasm.js    "$STAGE_DIR/pkg/"
cp pkg/typstuml_wasm_bg.wasm "$STAGE_DIR/pkg/"

cat > "$STAGE_DIR/README.md" <<'EOF'
# TypstUML Playground

Render PlantUML diagrams to SVG / PNG / PDF entirely in your browser.
No server, no installation, no account.

## Two ways to use this

**A. This zip — serve over HTTP** (smaller download, fast first load)

ES modules and WebAssembly can't load over `file://`, so this version
needs a static server. Run one of:

```sh
cd typstuml-playground
python3 -m http.server 8000   # then open http://localhost:8000
# or
npx serve
```

**B. The single-HTML version** (`typstuml-playground.html` if you got it)

Double-click. That's it. Larger file because the wasm is base64-inlined.

## What's inside

| File                          | Purpose                                |
|-------------------------------|----------------------------------------|
| `index.html`                  | The playground page                    |
| `pkg/typstuml_wasm.js`        | wasm-bindgen JavaScript shim           |
| `pkg/typstuml_wasm_bg.wasm`   | The compiled TypstUML module           |

## Features

- Live preview, debounced re-render on edits
- SVG / PNG / PDF output (PNG has a 1× / 2× / 3× / 4× resolution picker)
- On-demand fonts (Noto Sans SC, Noto Color Emoji) fetched from a CDN
  and cached in IndexedDB
- Zoom, download
- Fully offline once the page is loaded (except the optional fonts above)

Source: <https://github.com/daleione/typstuml>
EOF

OUT_ZIP="dist/${STAGE_NAME}.zip"
rm -f "$OUT_ZIP"
(cd "$(dirname "$STAGE_DIR")" && zip -qr - "$STAGE_NAME") > "$OUT_ZIP"
rm -rf "$STAGE_DIR"
rmdir "$(dirname "$STAGE_DIR")" 2>/dev/null || true

ZIP_SIZE=$(du -h "$OUT_ZIP" | cut -f1)
echo ">> $OUT_ZIP ($ZIP_SIZE)"

# ---------------------------------------------------------------------------
# 2) typstuml-playground.html — single file, double-click
# ---------------------------------------------------------------------------

OUT_HTML="dist/${STAGE_NAME}.html"

# Python handles base64 + multi-MB string assembly more cleanly than bash.
# Reads index.html, swaps out the `<script type="module"> import ... ;` block
# for an inlined no-modules shim + a tiny bootstrap that decodes the
# inlined wasm and reconstructs the imported names.
python3 - "$OUT_HTML" <<'PYEOF'
import base64, re, sys
from pathlib import Path

out_path = Path(sys.argv[1])
here = Path(".")
html      = (here / "index.html").read_text()
nomod_js  = (here / "pkg-nomod" / "typstuml_wasm.js").read_text()
wasm_b64  = base64.b64encode((here / "pkg-nomod" / "typstuml_wasm_bg.wasm").read_bytes()).decode("ascii")

bootstrap = f"""<script>
// --- wasm-bindgen no-modules shim (inlined verbatim) -----------------------
{nomod_js}
</script>
<script>
// --- standalone bootstrap: decode inlined wasm + alias the names the app
//     expects (matches the original ESM `import init, {{ ... }} from ...`). --
const WASM_B64 = "{wasm_b64}";
function _decodeB64(s) {{
  const bin = atob(s);
  const buf = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) buf[i] = bin.charCodeAt(i);
  return buf;
}}
const init      = (input) => wasm_bindgen({{ module_or_path: input ?? _decodeB64(WASM_B64) }});
const renderSvg = (...a) => wasm_bindgen.renderSvg(...a);
const renderPng = (...a) => wasm_bindgen.renderPng(...a);
const renderPdf = (...a) => wasm_bindgen.renderPdf(...a);
const emitTypst = (...a) => wasm_bindgen.emitTypst(...a);
const addFont   = (...a) => wasm_bindgen.addFont(...a);
"""

# Replace `<script type="module"> ... import init, {...} from "./pkg/...";`.
# The rest of the original module script body stays — its closing </script>
# at the end of the file closes our injected open <script> tag.
pat = re.compile(
    r'<script type="module">\s*'
    r'(?://[^\n]*\n\s*)*'                              # leading // comments
    r'import init, \{[^}]+\} from "\./pkg/typstuml_wasm\.js";'
)
# Use a lambda replacement instead of passing `bootstrap` as a string —
# re.sub processes backslash escapes in string replacements, which would
# turn the `\n` / `\u00xx` / `\"` sequences inside the inlined wasm-bindgen
# JS into actual newlines / unicode chars / unescaped quotes and corrupt
# the script. A lambda return is used verbatim.
new_html, n = pat.subn(lambda _m: bootstrap, html, count=1)
if n != 1:
    sys.exit("FAILED: did not find the expected `<script type=\"module\">…import…` block in index.html")

out_path.write_text(new_html)
print(f">> {out_path} ({out_path.stat().st_size / 1048576:.1f} MB)")
PYEOF
