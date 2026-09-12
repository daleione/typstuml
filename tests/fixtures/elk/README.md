# ELK regression fixtures

These checked-in snapshots are the reference inputs and expected outputs for
`tests/elk_port.rs`. Rust builds and tests read them directly and need neither
Node, npm dependencies nor the reference generator.

From the repository root:

```sh
cargo test --test elk_port
cargo run --example elk_compare -- tests/fixtures/elk/01-architecture.stages.json pass2
```

## Contents and provenance

| Files | Source and purpose |
| --- | --- |
| `01-architecture.stages.json` | [Architecture source](../component/01-architecture.puml); recorded draw-uml pipeline stages |
| `label-bench.stages.json` | [Label benchmark](label-bench.puml); recorded pipeline with edge labels |
| `xhs-flow.stages.json` | [Publishing source](../../../examples/content-publishing.puml); recorded pipeline with nested groups and a cycle |
| `{root-flat,stress-flat,dag-flat,label-flat,label-compound}.input.json` | Self-contained ELK graph inputs, including node sizes and layout options |
| Corresponding `*.output.json` | Expected elkjs layouts for those inputs |
| `stress-flat.bk.golden`, `stress-flat.e7.golden` | Historical instrumented elkjs snapshots for node placement and edge routing phases |

Stage snapshots contain the measured model, both ELK pass inputs and outputs,
the extracted layout and the final post-processed layout. Text sizes were
supplied by a deterministic measurement provider in the reference runner;
they are layout inputs, not measurements from native Typst fonts.

The pinned reference environment used to validate these snapshots is:

| Dependency | Version |
| --- | --- |
| `elkjs` | `0.11.1` |
| `@markdown-viewer/draw-uml` | `1.4.0` |
| `@markdown-viewer/text-measure` | `1.1.1` |
| `linkedom` | `0.18.13` |
| `tsx` | `4.23.1` |

## Updating reference data

Generate candidate layouts with the reference versions listed above and save
them under `tmp/`. Use the recorded graph inputs and preserve the deterministic
text measurement when regenerating draw-uml stages: changing input dimensions
changes the reference coordinates. Keep `meta.input` relative to the repository
root.

Review candidate differences, copy the accepted files into this directory,
then run the Rust tests above. Record changes to reference versions here.
Do not regenerate expected results from the Rust implementation under test.
The `.bk.golden` and `.e7.golden` files require the original phase instrumentation;
the standard elkjs runner cannot regenerate those historical intermediate dumps.
