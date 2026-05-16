# typstuml — PlantUML in Typst

Render PlantUML diagrams inline in a Typst document. No Java, no
Graphviz — a Rust → WebAssembly plugin does the parsing, layout, and
codegen, and Typst itself paints the result through the
[`blockcell`](https://github.com/daleione/blockcell) primitives bundled
in this package.

## Quick start

```typst
#import "@preview/typstuml:0.1.0": render-puml

= Demo

#render-puml(```
@startuml
Alice -> Bob: 你好
Bob --> Alice: 收到
@enduml
```.text)

#figure(
  render-puml(read("diagrams/login.puml")),
  caption: [Login sequence],
)
```

`read()` lives in user code (not in a `render-puml-file` helper) because
Typst sandboxes `read()` to the project root of the file that calls it.
A helper inside this package would look in the package install
directory, never your document.

Local development install (before the package is on `@preview`):

```sh
git clone --recurse-submodules https://github.com/daleione/typstuml
cd typstuml/crates/typstuml-plugin
./build.sh                       # build typstuml.wasm
./package.sh --install-local     # stage everything to @local
```

Then in your `.typ`:

```typst
#import "@local/typstuml:0.1.0": render-puml
```

## API

| Function              | Returns   | What it does                       |
| --------------------- | --------- | ---------------------------------- |
| `render-puml(source)` | `content` | Render a PlantUML source string    |

The returned value is regular Typst content — wrap it in `figure`,
`box`, `scale`, etc. as needed.

## What's supported

Mirrors the `typstuml` CLI: sequence, class, JSON, YAML, WBS, mind-map,
use-case, component, deployment, state, and activity diagrams. See the
main repository's `README.md` for the per-feature status matrix.

## What's not supported

- `!include` — the plugin has no filesystem. Compose multi-file inputs
  on the Typst side with `read()` if needed.
- `skinparam` themes from the host document — for v1 the diagram uses
  the styling baked into blockcell. Override the surrounding Typst
  text settings (`#set text(...)`) to influence the body font.

## License

MIT — same as the rest of the typstuml project.
