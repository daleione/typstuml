//! State-diagram codegen.
//!
//! Layout follows Graphviz `dot` (PlantUML's engine). Pipeline:
//!
//! 1. Heuristic per-node geometry (`node_geom`) — char-count width estimate
//!    for text-bearing states, fixed sizes for pseudostates.
//! 2. **Recursive cluster layout** (`layout_nodes`): each composite's
//!    interior is laid out as its own sub-graph (`layout_flat`), the
//!    resulting bbox fixes the composite's frame size, and that frame
//!    becomes a single box node in the parent level — so a composite's
//!    outside successors rank below the whole box and bypass edges route
//!    beside it (no post-hoc frame patches). Concurrent regions are
//!    sibling sub-layouts inside their composite.
//!    Within a level (`layout_flat`) the placer is dot's network simplex:
//!    rank assignment honours each edge's `minlen` (the dash count), x is
//!    NS on the auxiliary graph, and labelled edges carry a virtual label
//!    node that reserves rank + perpendicular space. PlantUML's single-dash
//!    `A -> B` is a *horizontal* link — `A`/`B` share a rank — so each
//!    maximal horizontal-linked component is **condensed** into one
//!    super-node for the rank pass and expanded back afterwards.
//! 3. Emit a single `#state-layout(...)` call with absolute coordinates;
//!    the painter draws shapes + edges + labels. Obstructed edges are
//!    detoured around composite frames by `route_transitions`.
//!
//! Self-loop transitions are kept out of the layout graph (the painter
//! draws them as an arc on the node itself) but still emitted so the
//! painter can render them.
//!
//! The codegen is split across submodules:
//! - [`geom`] — pass-1 measure probes and node / note geometry.
//! - [`route`] — geometric primitives and the obstacle-aware router.
//! - [`layout`] — the recursive cluster layout.
//! - [`emit`] — the `#state-layout(...)` serializer (entry point).

use std::fmt::Write as _;

mod emit;
mod geom;
mod layout;
mod route;
mod view;

pub use emit::emit;
pub use geom::{collect_probes, has_probes};

/// Escape a string for embedding in a Typst double-quoted literal.
fn typst_str_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Emit `key: "value", ` (escaped) or `key: none, ` into the painter call.
fn emit_opt_str(out: &mut String, key: &str, val: Option<&str>) {
    match val {
        Some(v) => write!(out, "{key}: \"{}\", ", typst_str_escape(v)).unwrap(),
        None => write!(out, "{key}: none, ").unwrap(),
    }
}
