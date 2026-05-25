//! Intermediate representation.
//!
//! The IR is the contract between parsers and codegen. Long-term goal is a
//! single shared "data → render" protocol so codegen never depends on
//! parser-specific shapes.
//!
//! Sequence diagrams ship in two flavors:
//!   - [`SequenceDiagram::Raw`] — body kept verbatim, parser does only a
//!     light hint scan; rendering is fully delegated to `blockcell.seq-puml`.
//!     Reserved as a bypass for future loose-mode error recovery; the
//!     current parser doesn't produce it.
//!   - [`SequenceDiagram::Structured`] — full AST built by the native Rust
//!     parser, with line-accurate metadata for diagnostics and a place for
//!     `skinparam` values to live before codegen translates them.
//!
//! Cuca diagrams (the class / component / deployment / use case family —
//! see `docs/cuca-diagram-design.md`) live in [`CucaDiagram`]. The shape
//! of each entity is selected by the [`USymbol`] enum (shared with
//! containers, mirroring PlantUML's `USymbols.java` registry), and the
//! shape-specific extras (class members, note body, object fields)
//! live in [`EntityKindData`]. The single [`Diagram::Cuca`] variant
//! covers what PlantUML internally calls `class`, `description`
//! (component / deployment / use case), and `object` diagrams.

mod activity;
mod common;
mod cuca;
mod record;
mod sequence;
mod state;
mod tree;

pub use activity::*;
pub use common::*;
pub use cuca::*;
pub use record::*;
pub use sequence::*;
pub use state::*;
pub use tree::*;

#[derive(Clone, Debug)]
pub struct Document {
    pub diagrams: Vec<Diagram>,
}

#[derive(Clone, Debug)]
pub enum Diagram {
    Sequence(SequenceDiagram),
    Json(JsonDiagram),
    Yaml(YamlDiagram),
    Wbs(WbsDiagram),
    MindMap(MindMapDiagram),
    Cuca(CucaDiagram),
    Activity(ActivityDiagram),
    State(StateDiagram),
}
