# TypstUML

Render PlantUML diagrams to SVG / PDF / PNG via Typst — no Java, no Graphviz.

**TypstUML** is a single-binary CLI that parses a subset of PlantUML and
renders it through [Typst](https://typst.app/) using the
[`blockcell`](https://github.com/daleione/blockcell) diagram primitives.
Cargo crate name and binary command: `typstuml`.

> Status: **M0 / early preview.** Sequence diagrams are the first supported
> diagram type. State, activity, mindmap, WBS, and JSON are planned next.
> See `docs/research/puml-typst-cli.md` in the parent `blockcell` repo for the
> full roadmap.

## Why

| Dimension       | Classic PlantUML            | TypstUML                           |
| --------------- | --------------------------- | ---------------------------------- |
| Runtime         | JRE + Graphviz / dot        | Single Rust binary, embedded Typst |
| Fonts / CJK     | AWT / Batik                 | Typst + HarfBuzz (great CJK)       |
| Output pipeline | Java rendering              | Typst compile → SVG / PDF / PNG    |
| Embeddable      | External process            | CLI today, library / WASM next     |

## Install (from source)

```sh
git clone --recurse-submodules https://github.com/daleione/typstuml.git
cd typstuml
cargo install --path .
```

If you cloned without `--recurse-submodules`, run
`git submodule update --init` first (`build.rs` will also try to do this
automatically inside a git checkout).

A pre-built single binary will be published once the M3 milestone ships.

## Usage

```sh
# Render to SVG (default format inferred from -o extension)
typstuml examples/hello.puml -o hello.svg

# Pick the format explicitly
typstuml examples/hello.puml -f pdf -o hello.pdf
typstuml examples/hello.puml -f png -o hello.png

# Read from stdin, write to stdout
cat examples/hello.puml | typstuml - --stdout -f svg > hello.svg

# Parse only, no render
typstuml --check examples/hello.puml

# Inspect the generated Typst source
typstuml --emit-typst examples/hello.puml
```

### Flags

| Flag                              | Purpose                                               |
| --------------------------------- | ----------------------------------------------------- |
| `-o, --output <PATH>`             | Output file (format inferred from extension)          |
| `-f, --format <svg\|pdf\|png>`    | Force output format                                   |
| `--stdout`                        | Write output to stdout instead of a file              |
| `--check`                         | Parse only — no rendering                             |
| `--emit-typst`                    | Print the generated Typst source instead of rendering |
| `--typst-template <FILE>`         | Inject a custom Typst preamble (fonts, themes, …)     |
| `--include <DIR>`                 | Add a search path for `!include` (repeatable)         |
| `--compat <strict\|warn\|loose>`  | How strict to be about unsupported PlantUML syntax    |

## What's supported in M0

The Sequence diagram coverage in M0 is whatever `blockcell`'s `seq-puml`
function already supports, since M0 routes the source through it directly:

- participant / actor / boundary / control / database / collections / queue / entity
- messages, self-calls, return arrows
- `alt` / `else` / `opt` / `loop` / `par` blocks
- `note over A`, `note over A, B`, multi-line notes
- `== divider ==`
- `autonumber` (start / stop / resume)
- `create` / `destroy`
- color overrides on participants and arrows

A native Rust parser with golden tests, richer diagnostics, and full
`skinparam` mapping lands in M1 — see the project roadmap.

## Layout

```
TypstUML/
  src/
    cli/         CLI argument parsing & orchestration
    parser/      Hand-written PlantUML parser (lexer / preprocessor / per-diagram)
    ir/          Normalized intermediate representation
    theme/       skinparam / !theme handling
    codegen/     IR → Typst source emission
    runtime/     Typst-as-library wrapper (TypstWorld + render to SVG/PDF/PNG)
  vendor/
    blockcell/   Git submodule → daleione/blockcell. `build.rs` stages
                 `lib.typ` + `src/` into `$OUT_DIR` for `include_dir!`.
  tests/
    fixtures/    Sample .puml inputs
    golden/      Expected outputs (snapshot tests, M1+)
```

### Updating the vendored `blockcell`

```sh
git submodule update --remote vendor/blockcell
git add vendor/blockcell && git commit -m "Bump blockcell"
```

## License

MIT. Vendored `blockcell` sources keep their original MIT license.
