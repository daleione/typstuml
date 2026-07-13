//! Sugiyama-style layered graph layout.
//!
//! Originally adapted from <https://github.com/nadavrot/layout> (`layout-rs`,
//! MIT — see `LICENSE` next to this file). The original was a full
//! DOT-renderer with its own SVG backend; we kept only the layout pipeline
//! (rank → mincross → Brandes-Kopf x-coords → edge straighten), expressed
//! in Typst pt directly. The painter lives Typst-side.
//!
//! Public surface:
//! - [`graph::VisualGraph`]: build a graph, call `layout()`, read positions
//!   and edges.
//! - [`pathplan`]: port of dot's `pathplan` library — constraint polygon
//!   construction, Lee-Preparata shortest path, B-spline routing — used by
//!   record-graph codegen for obstacle-aware edge routes.

pub mod dag;
pub mod geometry;
pub mod graph;
pub mod ortho;
pub mod pathplan;
pub mod spacing;
pub mod sugiyama;
pub mod swimlane;
