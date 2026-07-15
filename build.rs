//! `components/` holds a hand-curated subset of the blockcell Typst library
//! (upstream: https://github.com/daleione/blockcell) — just the sources
//! TypstUML's codegen actually calls into, embedded into the binary via
//! `include_dir!` at `src/runtime/world.rs`.
//!
//! ## What the codegen emits, and what blockcell symbols that touches
//!
//! `src/codegen/record_graph.rs`, `src/codegen/sequence.rs`,
//! `src/codegen/wbs.rs`, and `src/codegen/class.rs` together emit these
//! blockcell calls:
//!
//! ```text
//!   #record-layout(...)   — JSON / YAML record diagrams
//!                           (components/src/records.typ)
//!   #seq-puml(...)        — sequence diagrams
//!                           (components/src/seq-puml.typ)
//!   #tree(...) / #node[…] — WBS diagrams
//!   #mindmap(...)         — mind-map diagrams
//!                           (components/src/tree.typ)
//!   #cuca-layout(...)     — cuca diagrams (class / component /
//!                           deployment / use case)
//!                           (components/src/cuca.typ)
//! ```
//!
//! `record-layout` only depends on private helpers inside `records.typ`.
//! `seq-puml` pulls in `seq.typ` and `palettes.typ` transitively, and
//! both `seq.typ` and `records.typ` further reach into
//! `internal/metrics.typ`. `tree.typ` only needs `palettes.typ`.
//!
//! Activity diagrams add:
//!
//! ```text
//!   #flow-col(...)        — vertical step composition
//!   #branch-merge(...)    — if-else with rejoining branches
//!   #switch(...)          — N-way diamond fan-out
//!   #fork-bar(...)        — concurrent fork / split with sync-bars
//!   #flow-loop(...)       — while / repeat back-edge
//!   #process / #decision  — action / decision atoms
//!   #start-marker / #stop-marker / #end-marker / #detach-marker
//! ```
//!
//! These live in `flows.typ` + `composites.typ::flow-col` +
//! `atoms.typ`, transitively pulling in `containers.typ` and
//! `internal/stroke.typ`.
//!
//! State diagrams add:
//!
//! ```text
//!   #state-layout(...)   — UML state machines
//!                          (components/src/states.typ)
//! ```
//!
//! `states.typ` only depends on `palettes.typ`.
//!
//! `components/` is a plain tracked directory in this repo — not a
//! submodule. See CLAUDE.md ("Modifying blockcell") for how to sync
//! changes with the upstream blockcell working tree.

fn main() {
    println!("cargo:rerun-if-changed=components");
}
