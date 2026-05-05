//! Intermediate representation.
//!
//! The IR is the contract between parsers and codegen. Per design doc §14
//! item 3, the long-term goal is a single shared "data → render" protocol so
//! that codegen never depends on parser-specific shapes. M0 only models
//! Sequence diagrams, and only as an opaque raw payload — but the enum is
//! already shaped so future diagram types slot in without reworking the
//! pipeline.

#[derive(Clone, Debug)]
pub struct Document {
    pub diagrams: Vec<Diagram>,
}

#[derive(Clone, Debug)]
pub enum Diagram {
    Sequence(SequenceDiagram),
    // Future: State(StateDiagram), Activity(...), MindMap(TreeDiagram), ...
}

#[derive(Clone, Debug)]
pub enum SequenceDiagram {
    /// M0 holds the body verbatim and lets `blockcell.seq-puml` parse it on
    /// the Typst side. The hints come from a quick parser-side scan and
    /// drive the codegen width heuristic — both disappear in M1 once the
    /// native Sequence parser produces a `Structured` variant.
    Raw {
        name: Option<String>,
        title: Option<String>,
        body: String,
        hints: SequenceHints,
    },
    // Future: Structured(StructuredSequence) — populated by the M1 native parser.
}

#[derive(Clone, Debug, Default)]
pub struct SequenceHints {
    /// Number of declared participants (or, if none are declared,
    /// distinct endpoints implied by arrow lines, clamped to a minimum).
    pub participants: u32,
    /// Longest message label seen (in characters), used to pad the
    /// codegen width estimate when labels are unusually long.
    pub max_label_chars: u32,
}
