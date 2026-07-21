//! Web display-list emitters — the JSON contract between the Rust
//! layout and browser-side renderers.
//!
//! Design per `docs/mindmap-web-interactive-design.md` §4: the browser
//! measures node sizes (its own font engine is the ground truth for
//! what it renders), Rust computes the layout. Two entry points:
//!
//! - [`tree::model_json`] — one-time: parse a `.puml` source into a
//!   structural tree model (labels, shapes, colors, stable IDs — no
//!   geometry).
//! - [`tree::display_list_json`] — per interaction: model + measured
//!   sizes + folded set → absolute coordinates. Pure arithmetic; never
//!   touches Typst, so the fold/relayout loop stays instant.

pub mod tree;
