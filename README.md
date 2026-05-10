# TypstUML

Render PlantUML diagrams to SVG / PDF / PNG via Typst — no Java, no Graphviz.

**TypstUML** is a single-binary CLI that parses a subset of PlantUML and
renders it through [Typst](https://typst.app/) using the
[`blockcell`](https://github.com/daleione/blockcell) diagram primitives.
Cargo crate name and binary command: `typstuml`.

> Active development. Sequence, JSON, YAML, WBS, mind-map, and class
> diagrams (with packages, notes, lollipops, association classes,
> orthogonal edges, and `!theme`) render today; other diagram types are
> recognized by the parser but not yet wired up. CLI is subcommand-based
> with a `watch` mode for live re-rendering. See
> [Features](#features) for the full status matrix.

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

The CLI is subcommand-based. `compile` is the default — running
`typstuml <input> [output]` with no subcommand renders the input.
Use `-` (or omit the output) to write to stdout.

```sh
# Compile (implicit). Output is positional; format is inferred from extension.
typstuml diagram.puml diagram.svg
typstuml diagram.puml diagram.pdf
typstuml diagram.puml diagram.png

# Force the output format
typstuml -f pdf diagram.puml diagram.pdf

# Stdin → stdout (Unix pipe friendly)
cat diagram.puml | typstuml - > diagram.svg

# Parse only — exit non-zero on parse errors
typstuml check diagram.puml

# Print the generated Typst source instead of rendering
typstuml emit diagram.puml

# Re-render on every save; tracks the input plus every !include'd file
typstuml watch diagram.puml diagram.svg

# List supported diagram types
typstuml diagrams
```

### Commands

| Command              | Purpose                                                          |
| -------------------- | ---------------------------------------------------------------- |
| `compile` (default)  | Render a `.puml` to SVG / PDF / PNG                              |
| `check`              | Parse only — exit non-zero on parse errors                       |
| `emit`               | Print the generated Typst source instead of rendering            |
| `watch`              | Initial render, then re-render on every save (input + includes)  |
| `diagrams`           | List supported diagram types                                     |

### Options

| Flag                              | Scope          | Purpose                                                  |
| --------------------------------- | -------------- | -------------------------------------------------------- |
| `-f, --format <svg\|pdf\|png>`    | compile, watch | Force the output format                                  |
| `-I, --include <DIR>`             | global         | Search path for `!include`, repeatable                   |
| `--compat <strict\|warn\|loose>`  | global         | Strictness for unsupported PlantUML syntax (default `warn`) |
| `-q, --quiet`                     | global         | Suppress informational stderr (warnings still shown)     |
| `-v, --verbose`                   | global         | Verbose stderr output                                    |

### Watch mode

`typstuml watch <input> <output>` does an initial render, subscribes to
the input file's parent directory plus the parent directory of every
`!include`d file, then re-renders (debounced ~150 ms) whenever a tracked
file changes. Most external SVG / PDF viewers will auto-reload the
output. Parse and render errors are reported but do not exit the
watcher — fix the source and save again.

## Features

Legend: ✅ shipped · 🚧 partial · ⏳ planned

| Diagram                       | Status | Notes                                                                                   |
| ----------------------------- | :----: | --------------------------------------------------------------------------------------- |
| Sequence (`@startuml`)        |   ✅   | Lifelines, messages, fragments, notes, `autonumber`, `create` / `destroy` — native parser |
| JSON (`@startjson`)           |   ✅   | Linked record blocks with dashed reference arrows; `☑ true` / `☒ false` / `␀` markers   |
| YAML (`@startyaml`)           |   ✅   | Shares the JSON `record-graph` renderer; flow & block style, anchors / aliases via serde |
| MindMap (`@startmindmap`)     |   ✅   | Left/right fan-out via `blockcell.mindmap`                                              |
| WBS (`@startwbs`)             |   ✅   | Work-breakdown hierarchy                                                                |
| Class                         |   ✅   | 3-compartment cards, 7 head shapes, packages / namespaces / together, lollipop, notes (anchored / multi-target / on-link), `hide` filters, custom stereotype markers, generic corner box, association class, edge inline color, member ports, Manhattan edges, Creole-lite markup, `!theme` builtins, `left to right direction` |
| `skinparam` coverage          |   🚧   | `backgroundColor`, `defaultFontName`, `defaultFontSize` map today; rest pass through    |
| Object                        |   ⏳   | UML object instances with field values                                                  |
| Component                     |   ⏳   | Components, interfaces, ports                                                           |
| Deployment                    |   ⏳   | Nodes, artifacts, devices                                                               |
| Use case                      |   ⏳   | Actors + ellipses inside a system boundary                                              |
| State                         |   ⏳   | UML state machines with transitions — planned via `blockcell.state-chain`               |
| Activity                      |   ⏳   | Flowcharts / workflows — planned via `blockcell.flow-col`                               |
| Timing                        |   ⏳   | Concurrent lifelines + state transitions over time                                      |
| Gantt (`@startgantt`)         |   ⏳   | Project schedules with date axis                                                        |
| Salt (`@startsalt`)           |   ⏳   | UI / wireframe mockups                                                                  |
| Network (`nwdiag`)            |   ⏳   | Network topology                                                                        |
| Ditaa (`@startditaa`)         |   ⏳   | ASCII-art passthrough                                                                   |
### Updating the vendored `blockcell`

```sh
git submodule update --remote vendor/blockcell
git add vendor/blockcell && git commit -m "Bump blockcell"
```

## License

MIT. Vendored `blockcell` sources keep their original MIT license.
