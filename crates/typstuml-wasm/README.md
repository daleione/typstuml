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

The module bundles the whole Typst compiler and its default fonts, so it's
big — but very compressible. Current build: **~27 MB raw, ~13 MB gzipped**.
Serve it with gzip/brotli (`python3 -m http.server` does *not* compress; a
real static host or CDN will). The biggest remaining lever is trimming the
embedded font set in the workspace `Cargo.toml`.

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

The first page load fetches the `.wasm` module (~27 MB uncompressed), so give
it a few seconds. After that, editing the source re-renders automatically.

To stop the server, press `Ctrl-C` in its terminal. If you started it in the
background, stop it with:

```sh
pkill -f "http.server 8000"
```

Any static server works — e.g. `npx serve` or `python3 -m http.server` on a
different port. Just serve this directory so `./pkg/` is reachable.

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
