# typstuml-wasm

WebAssembly bindings for [TypstUML](../../) — render PlantUML diagrams to
SVG / PNG / PDF entirely in the browser (or any `wasm32` host).

The whole pipeline is self-contained: the parsers, the Sugiyama layout, the
embedded `blockcell` Typst sources, the Typst compiler, and Typst's default
fonts are all baked into the `.wasm` module. No network, no filesystem.

## What's exposed

| JS function            | Returns                | Notes                                  |
|------------------------|------------------------|----------------------------------------|
| `renderSvg(source)`    | `string`               | Primary entry point for web use.       |
| `renderPng(source)`    | `Uint8Array`           | First diagram only on multi-diagram input. |
| `renderPdf(source)`    | `Uint8Array`           |                                        |
| `emitTypst(source)`    | `string`               | The generated Typst source — for debugging. |
| `addFont(bytes)`       | `number` (face count)  | Append a TTF/OTF/TTC to the font book at runtime — see [Extra fonts](#extra-fonts). |

All throw a JS `Error` with a line-annotated diagnostic message on failure.

Not supported in the wasm build: `!include` directives (no filesystem) and
on-disk user preambles. Use the native CLI for those.

## Building

Prerequisites — the wasm target, plus the `wasm-bindgen` and `wasm-opt` CLIs:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli   # version must match the wasm-bindgen crate
brew install binaryen            # or: cargo install wasm-opt
```

Then, from this directory:

```sh
./build.sh
```

`build.sh` runs three stages and writes `pkg/` (the `.wasm` module plus JS
glue). It deliberately drives `cargo` and `wasm-bindgen` directly rather than
using `wasm-pack` — see the script header for the full reasoning.

1. **`cargo build --profile wasm-release`** — the Rust build under the
   `wasm-release` cargo profile (defined in the workspace root: `opt-level=z`,
   fat LTO, `strip`, `panic=abort`). It's a *custom* profile, so native
   `cargo build --release` for the CLI is completely unaffected.
2. **`wasm-bindgen --target web`** — generates `pkg/typstuml_wasm.js` and
   `pkg/typstuml_wasm_bg.wasm`.
3. **`wasm-opt -Oz --all-features`** — binaryen's size pass. `--all-features`
   is mandatory: rustc emits post-MVP wasm (bulk-memory, sign-extension,
   non-trapping float→int, …) that wasm-opt rejects otherwise.

Both CLIs are looked up on `PATH` first, then in wasm-pack's download cache
(handy if you've run `wasm-pack` before). If `wasm-opt` is missing,
`./build.sh --no-opt` skips stage 3 — the module just stays larger.

To only compile-check:

```sh
cargo build -p typstuml-wasm --target wasm32-unknown-unknown --profile wasm-release
```

### Size

The module bundles the whole Typst compiler plus the two fonts the compiler
needs at layout time — but the wasm is very compressible.

| transport | size |
|-----------|------|
| raw `.wasm`         | ~20 MB |
| gzip                | ~8.5 MB |
| brotli (CDN)        | ~6.1 MB |

`python3 -m http.server` does **not** compress; a real static host or CDN
will. The two embedded fonts (`LibertinusSerif-Regular.otf` and
`DejaVuSansMono.ttf`, ~660 KB combined) live under `src/runtime/fonts/` in the
main crate — bumped through `include_bytes!` only on `target_arch = "wasm32"`,
so the native CLI is unaffected. See `src/runtime/fonts/NOTICE` for upstream
licenses (OFL / Bitstream Vera).

## Extra fonts

The two embedded fonts cover Latin only. Rendering CJK / emoji / cyrillic
labels needs additional fonts, but bundling them into the `.wasm` would
double or triple the download — a CJK font on its own is 5–10 MB.

The solution is `addFont(bytes)`: the JS host fetches font files when it
actually needs them and pushes the bytes into the wasm at runtime. Typst's
automatic fallback then uses them whenever the embedded defaults don't
cover a requested glyph. The included playground does this end-to-end:

1. **"Fonts" button** in the header opens a popover listing built-in and
   downloadable fonts.
2. Clicking *Load* fetches the font from a CDN with a progress bar.
3. Bytes are persisted to **IndexedDB** so repeat visits don't re-download.
4. After load, the current diagram re-renders automatically and any
   previously-broken non-Latin glyphs appear.

The shipping catalogue has two entries — both via `cdn.jsdelivr.net/gh/`:

| Font            | Use                  | Size  | Source repo                |
|-----------------|----------------------|-------|----------------------------|
| Noto Sans SC    | Simplified Chinese   | ~8 MB | `notofonts/noto-cjk`       |
| Noto Color Emoji | Color emoji (COLRv1) | ~5 MB | `googlefonts/noto-emoji`   |

To add more (Japanese, Korean, …) edit `AVAILABLE_FONTS` in `index.html`.
Picking URLs: look for OTF/TTF files served with `Access-Control-Allow-Origin: *`
and an `Etag` or strong `Cache-Control` — jsdelivr-gh and the
notofonts.github.io mirror both satisfy this. **WOFF2 won't work** without
a JS-side decoder; Typst expects raw font tables. For emoji, prefer the
**COLRv1** variant (vector) over the legacy CBDT `NotoColorEmoji.ttf`
(bitmap, ~10 MB): COLRv1 renders as crisp vector paths in SVG output.

Embedders not using the playground can call `addFont` directly:

```js
import init, { addFont, renderSvg } from "./pkg/typstuml_wasm.js";
await init();

const bytes = new Uint8Array(await (await fetch("/fonts/NotoSansSC.otf")).arrayBuffer());
const faces = addFont(bytes);     // throws on invalid font; returns 1+ on success
const svg = renderSvg("@startjson\n{ \"项目\": \"TypstUML\" }\n@endjson");
```

The wasm font cache is process-global — call `addFont` once at startup,
then every subsequent render sees the new fonts. There's no `removeFont`
counterpart: Typst's compile cache holds Font references for the life of
the module instance.

## Playground

`index.html` in this directory is a self-contained playground — a PlantUML
editor on the left, the rendered diagram on the right.

ES modules and wasm can't load from `file://`, so the directory has to be
served over HTTP. From this directory (`crates/typstuml-wasm/`):

```sh
# 1. Build the wasm package — writes ./pkg/ (skip if already built).
./build.sh

# 2. Start a static file server on port 8000.
python3 -m http.server 8000

# 3. Open the playground.
open http://localhost:8000          # macOS  (Linux: xdg-open, Windows: start)
```

The first page load fetches the `.wasm` module (~20 MB uncompressed, ~6 MB
brotli-compressed), so give it a few seconds. After that, editing the source
re-renders automatically.

To stop the server, press `Ctrl-C` in its terminal. If you started it in the
background, stop it with:

```sh
pkill -f "http.server 8000"
```

Any static server works — e.g. `npx serve` or `python3 -m http.server` on a
different port. Just serve this directory so `./pkg/` is reachable.

### Sharing the playground

`./package.sh` rolls `index.html` + `pkg/` (plus a brief how-to-run README)
into `dist/typstuml-playground.zip` — about 8.5 MB. Hand it off; the
recipient unzips, points a static server at the unzipped directory, and
opens it in any modern browser. No build toolchain required on their end.

## Usage

```js
import init, { renderSvg } from "./pkg/typstuml_wasm.js";

await init();

const svg = renderSvg(`
@startjson
{ "name": "TypstUML", "targets": ["native", "wasm"] }
@endjson
`);

document.getElementById("diagram").innerHTML = svg;
```
