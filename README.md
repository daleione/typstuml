# TypstUML

Render PlantUML and Mermaid flowcharts to SVG / PDF / PNG via Typst — no Java, no Graphviz.

**TypstUML** is a single-binary CLI that parses subsets of PlantUML and Mermaid and
renders it through [Typst](https://typst.app/) using the
[`blockcell`](https://github.com/daleione/blockcell) diagram primitives.
Cargo crate name and binary command: `typstuml`.

> Active development. Sequence, JSON, YAML, WBS, mind-map, class, and
> activity diagrams render today; other diagram types are recognized
> by the parser but not yet wired up. CLI is subcommand-based with a
> `watch` mode for live re-rendering. See [Features](#features) for
> the full status matrix.

## Why

| Dimension       | Classic PlantUML            | TypstUML                           |
| --------------- | --------------------------- | ---------------------------------- |
| Runtime         | JRE + Graphviz / dot        | Single Rust binary, embedded Typst |
| Fonts / CJK     | AWT / Batik                 | Typst + HarfBuzz (great CJK)       |
| Output pipeline | Java rendering              | Typst compile → SVG / PDF / PNG    |
| Embeddable      | External process            | CLI today, library / WASM next     |

## Install (from source)

```sh
git clone https://github.com/daleione/typstuml.git
cd typstuml
cargo install --path .
```

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

### Mermaid flowcharts

```sh
typstuml flow.mmd flow.svg             # .mmd / .mermaid selects Mermaid
typstuml --lang mermaid check flow.txt
typstuml --lang mermaid emit flow.txt
typstuml --lang mermaid - flow.pdf     # source from stdin
typstuml watch flow.mmd flow.svg
```

Supported: `flowchart`/`graph`, TD/TB/BT/LR/RL, rectangle, rounded, stadium,
diamond, circle, cylinder and asymmetric nodes; `-->`, `---`, `-.->`, `==>`;
pipe/inline edge labels, chains, nested subgraphs and Unicode labels.
Labels are plain text; repeated labels use the last explicit value. Quote
labels containing reserved punctuation. Mermaid input does not run the
PlantUML preprocessor, and `--include` is rejected.

Not yet supported: other Mermaid diagram types, HTML/Markdown labels,
styles/classes, click callbacks, init directives, new `@{shape: ...}` syntax,
subgraph endpoints, per-subgraph direction, shape redefinitions and ambiguous
multi-subgraph membership. IDs start with an ASCII letter or `_` and may
continue with letters, digits, `_` or `-`. Nonrectangular nodes use fixed
contour connection points; output follows this project's theme and layout.

CLI `--compat strict` rejects unsupported statements; `warn` reports and
skips a whole unsupported statement; `loose` skips it silently. Structural
errors always fail. Rust convenience APIs, browser `*As` APIs and Typst
`render-mermaid` use strict parsing.

```rust
use typstuml::{parser::InputLanguage, render::{render_source_with_language, Format}};
let svg = render_source_with_language("flowchart LR; A --> B", InputLanguage::Mermaid, Format::Svg)?;
```

In the Typst package: `#render-mermaid("flowchart LR; A --> B")`. Its v2
measurement protocol isolates each render instance, including repeated content
under different text styles. Existing PlantUML entry points keep their meaning.

### Commands

| Command              | Purpose                                                          |
| -------------------- | ---------------------------------------------------------------- |
| `compile` (default)  | Render `.puml` / `.mmd` to SVG / PDF / PNG                              |
| `check`              | Parse only — exit non-zero on parse errors                       |
| `emit`               | Print the generated Typst source instead of rendering            |
| `watch`              | Initial render, then re-render on every save (input + includes)  |
| `diagrams`           | List supported diagram types                                     |

### Options

| Flag                              | Scope          | Purpose                                                  |
| --------------------------------- | -------------- | -------------------------------------------------------- |
| `-f, --format <svg\|pdf\|png>`    | compile, watch | Force the output format                                  |
| `-I, --include <DIR>`             | global         | Search path for `!include`, repeatable                   |
| `--compat <strict\|warn\|loose>`  | global         | Strictness for unsupported syntax (default `warn`) |
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
| Class                         |   ✅   | Compartmented cards, packages / namespaces, lollipops, notes, association classes, Manhattan edges, `!theme`, `left to right direction` |
| `skinparam` coverage          |   🚧   | `backgroundColor`, `defaultFontName`, `defaultFontSize` map today; rest pass through    |
| Object                        |   ✅   | Instance cards with underlined name + `name = value` field rows                         |
| Component                     |   ⏳   | Components, interfaces, ports                                                           |
| Deployment                    |   ⏳   | Nodes, artifacts, devices                                                               |
| Use case                      |   ✅   | Actors + ellipses inside a system boundary                                              |
| Mermaid flowchart             |   ✅   | Native Rust subset: seven shapes, four edge styles, nested subgraphs, five directions |
| State                         |   ✅   | UML state machines                                                                      |
| Activity (`activitydiagram3`) |   ✅   | Structured flow: `if`/`while`/`repeat`/`fork`/`switch`, partitions, notes, swimlanes, 4 SDL shapes |
| Timing                        |   ⏳   | Concurrent lifelines + state transitions over time                                      |
| Gantt (`@startgantt`)         |   ⏳   | Project schedules with date axis                                                        |
| Salt (`@startsalt`)           |   ⏳   | UI / wireframe mockups                                                                  |
| Network (`nwdiag`)            |   ⏳   | Network topology                                                                        |
| Ditaa (`@startditaa`)         |   ⏳   | ASCII-art passthrough                                                                   |
## Architecture

Parsers produce diagram-specific semantic IR. Mermaid flowcharts use
`FlowchartDiagram` and `codegen/flowchart`; PlantUML class and component
diagrams use `CucaDiagram` and `codegen/cuca`.

`codegen/graph` shares geometry, compound layout, anchor constraints, routing,
and label placement. It has no parser or semantic IR dependencies. On the
Typst side, separate flowchart and CUCA painters use `src/graph/canvas.typ`.

CLI and library/WASM rendering share `render/pipeline`. `RenderOptions`
selects parsing compatibility and measurement policy (`Required`,
`BestEffort`, or `Disabled`); `CodegenOptions` independently selects document
or embedded output and default or inherited typography. Existing convenience
APIs retain their defaults; `render_source_with_options` and
`emit_typst_with_options` expose explicit control. Plugin v1 and v2 wire
adapters live separately in `crates/typstuml-plugin/src/`.

### Updating `components/` (the vendored `blockcell` subset)

`components/` is a hand-curated, plain-tracked copy of the
[`blockcell`](https://github.com/daleione/blockcell) Typst sources
TypstUML's codegen actually calls into — see `build.rs` and
`CLAUDE.md` for the full sync procedure with the upstream repo.

## Development tools

See [examples](examples/README.md) for diagram sources and the tree parity
exporter, and [ELK regression fixtures](tests/fixtures/elk/README.md) for reference
data, provenance and update instructions. These fixtures are checked in; running
the Rust tests does not require the external JavaScript generator. Generated
outputs should go under `tmp/` or `out/`.

## License

MIT. `components/` (vendored `blockcell` sources) keeps its original MIT license.
