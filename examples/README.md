# Examples and development tools

Render a diagram from the repository root:

```sh
cargo run -- examples/content-publishing.puml tmp/content-publishing.svg
cargo run -- tests/fixtures/mermaid/node-shapes.mmd tmp/mermaid.svg
```

| File | Purpose |
| --- | --- |
| `class.puml` | Class diagram |
| `flow.puml` | Activity flow |
| `m3-*.puml` | Cluster layout and sibling ordering examples |
| `content-publishing.puml` | Components, services and labeled publishing relationships; also an ELK regression source |
| `content-publishing-activity.puml` | The publishing process as an activity diagram with swimlanes |
| `elk_compare.rs` | Compare the production ELK pipeline with a recorded reference pass |
| `export_tree_fixtures.rs` | Export deterministic tree models, sizes and folded display lists for cross-language parity testing |

```sh
cargo run --example elk_compare -- tests/fixtures/elk/01-architecture.stages.json pass2
cargo run --example export_tree_fixtures -- tmp/tree-parity
```

Mermaid regression sources and expected generated Typst live together under
`tests/fixtures/mermaid` and `tests/golden/mermaid`. The Typst plugin example
is in `crates/typstuml-plugin/package/examples/mermaid.typ`; run it from an
assembled package so its plugin and component imports are available.
