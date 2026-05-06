# TypstUML

Render PlantUML diagrams to SVG / PDF / PNG via Typst — no Java, no Graphviz.

**TypstUML** is a single-binary CLI that parses a subset of PlantUML and
renders it through [Typst](https://typst.app/) using the
[`blockcell`](https://github.com/daleione/blockcell) diagram primitives.
Cargo crate name and binary command: `typstuml`.

> Active development. Sequence and JSON diagrams render today; other
> diagram types are recognized by the parser but not yet wired up. See
> the [Features](#features) section for the full status matrix.

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

## Features

Legend: ✅ shipped · 🚧 partial · ⏳ planned

| Diagram                       | Status | Notes                                                                                   |
| ----------------------------- | :----: | --------------------------------------------------------------------------------------- |
| Sequence (`@startuml`)        |   ✅   | Lifelines, messages, fragments, notes, `autonumber`, `create` / `destroy` — native parser |
| JSON (`@startjson`)           |   ✅   | Linked record blocks with dashed reference arrows; `☑ true` / `☒ false` / `␀` markers   |
| YAML (`@startyaml`)           |   ⏳   | Same data shape as JSON; will reuse the `record-graph` renderer                         |
| Class                         |   ⏳   | UML class boxes with fields / methods and inheritance / composition arrows              |
| Activity                      |   ⏳   | Flowcharts / workflows — planned via `blockcell.flow-col`                               |
| State                         |   ⏳   | UML state machines with transitions — planned via `blockcell.state-chain`               |
| Use case                      |   ⏳   | Actors + ellipses inside a system boundary                                              |
| Object                        |   ⏳   | UML object instances with field values                                                  |
| Component                     |   ⏳   | Components, interfaces, ports                                                           |
| Deployment                    |   ⏳   | Nodes, artifacts, devices                                                               |
| Timing                        |   ⏳   | Concurrent lifelines + state transitions over time                                      |
| MindMap (`@startmindmap`)     |   ⏳   | Radial tree — planned via `blockcell.tree`                                              |
| WBS (`@startwbs`)             |   ⏳   | Work-breakdown hierarchy                                                                |
| Gantt (`@startgantt`)         |   ⏳   | Project schedules with date axis                                                        |
| Salt (`@startsalt`)           |   ⏳   | UI / wireframe mockups                                                                  |
| Network (`nwdiag`)            |   ⏳   | Network topology                                                                        |
| Ditaa (`@startditaa`)         |   ⏳   | ASCII-art passthrough                                                                   |
| Full `skinparam` coverage     |   🚧   | `backgroundColor`, `defaultFontName`, `defaultFontSize` map today; rest pass through    |

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
    golden/      Expected outputs (snapshot tests)
```

### Updating the vendored `blockcell`

```sh
git submodule update --remote vendor/blockcell
git add vendor/blockcell && git commit -m "Bump blockcell"
```

## License

MIT. Vendored `blockcell` sources keep their original MIT license.
