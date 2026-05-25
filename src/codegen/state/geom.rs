//! Pass-1 measure probes and node / note geometry.
//!
//! `node_geom` / `note_geom` estimate text width from a char count, which is
//! wrong for proportional fonts and CJK. When a `MeasurementSet` is supplied,
//! `resolve_node_geom` / `resolve_note_size` use the painter-measured size
//! instead (see `state-probe` / `state-note-probe` in `states.typ`).

use std::fmt::Write as _;

use crate::ir::{StateDiagram, StateKind, StateNode, Transition};
use crate::layout::geometry::Point;
use crate::runtime::MeasurementSet;

use super::{emit_opt_str, typst_str_escape};

/// Heuristic average glyph advance at the 10pt body size.
const CHAR_W_PT: f64 = 6.2;
/// Heuristic glyph advance for `entry/exit/do` body rows (0.82em).
const BODY_CHAR_W_PT: f64 = 5.1;

pub(super) struct NodeGeom {
    pub(super) size: Point,
}

/// Stable probe id for a simple / composite state.
fn state_node_id(diagram_idx: usize, node: &StateNode) -> String {
    format!("ms-{diagram_idx}-{}", sanitize_id(&node.id))
}

/// Stable probe id for a note (notes have no user id, so key by index).
fn state_note_id(diagram_idx: usize, note_idx: usize) -> String {
    format!("msn-{diagram_idx}-{note_idx}")
}

/// Stable probe id for a transition's edge label (keyed by index).
fn state_edge_label_id(diagram_idx: usize, ti: usize) -> String {
    format!("mse-{diagram_idx}-{ti}")
}

/// Collapse an IR node id into a string safe to embed in a probe id.
fn sanitize_id(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// True iff the diagram has text-bearing content worth measuring — any
/// simple / composite state, or any note. Pseudostates are fixed-size.
pub fn has_probes(diag: &StateDiagram) -> bool {
    diag.nodes
        .iter()
        .any(|n| matches!(n.kind, StateKind::Simple | StateKind::Composite))
        || !diag.notes.is_empty()
        || diag.transitions.iter().any(has_edge_label)
}

/// True iff the transition carries an `event [guard] / action` label.
fn has_edge_label(tr: &Transition) -> bool {
    [
        tr.event.as_deref(),
        tr.guard.as_deref(),
        tr.action.as_deref(),
    ]
    .iter()
    .any(|p| p.is_some_and(|s| !s.is_empty()))
}

/// Emit one `#state-probe(...)` per simple / composite state and one
/// `#state-note-probe(...)` per note into the pass-1 source.
pub fn collect_probes(
    diag: &StateDiagram,
    diagram_idx: usize,
    out: &mut String,
    expected_ids: &mut Vec<String>,
) {
    for node in &diag.nodes {
        if !matches!(node.kind, StateKind::Simple | StateKind::Composite) {
            continue;
        }
        let id = state_node_id(diagram_idx, node);
        write!(
            out,
            "#state-probe(id: \"{}\", display: \"{}\", body: (",
            id,
            typst_str_escape(&node.display)
        )
        .unwrap();
        for (i, row) in node.body.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write!(out, "\"{}\"", typst_str_escape(row)).unwrap();
        }
        if node.body.len() == 1 {
            out.push(',');
        }
        out.push_str("))\n");
        expected_ids.push(id);
    }
    for (ni, note) in diag.notes.iter().enumerate() {
        let id = state_note_id(diagram_idx, ni);
        writeln!(
            out,
            "#state-note-probe(id: \"{}\", body: \"{}\")",
            id,
            typst_str_escape(&note.body)
        )
        .unwrap();
        expected_ids.push(id);
    }
    for (ti, tr) in diag.transitions.iter().enumerate() {
        if !has_edge_label(tr) {
            continue;
        }
        let id = state_edge_label_id(diagram_idx, ti);
        write!(out, "#state-edge-label-probe(id: \"{id}\", ").unwrap();
        emit_opt_str(out, "event", tr.event.as_deref());
        emit_opt_str(out, "guard", tr.guard.as_deref());
        emit_opt_str(out, "action", tr.action.as_deref());
        out.push_str(")\n");
        expected_ids.push(id);
    }
}

/// Per-node geometry: measured size from pass-1 when available, otherwise
/// the char-count heuristic. Pseudostates are always fixed-size.
pub(super) fn resolve_node_geom(
    n: &StateNode,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> NodeGeom {
    if matches!(n.kind, StateKind::Simple | StateKind::Composite) {
        if let Some(set) = measurements {
            if let Some(m) = set.get(&state_node_id(diagram_idx, n)) {
                return NodeGeom {
                    size: Point::new(m.width_pt, m.height_pt),
                };
            }
        }
    }
    node_geom(n)
}

/// Note sticky size: measured from pass-1 when available, else heuristic.
pub(super) fn resolve_note_size(
    note_idx: usize,
    body: &str,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> Point {
    if let Some(set) = measurements {
        if let Some(m) = set.get(&state_note_id(diagram_idx, note_idx)) {
            return Point::new(m.width_pt, m.height_pt);
        }
    }
    note_geom(body)
}

/// Heuristic bounding box for one node.
fn node_geom(n: &StateNode) -> NodeGeom {
    let size = match n.kind {
        StateKind::Initial | StateKind::Final => Point::new(18.0, 18.0),
        StateKind::EntryPoint | StateKind::ExitPoint => Point::new(12.0, 12.0),
        StateKind::History | StateKind::DeepHistory => Point::new(24.0, 24.0),
        StateKind::Choice => Point::new(32.0, 32.0),
        StateKind::Fork | StateKind::Join => Point::new(70.0, 10.0),
        StateKind::SynchroBar => Point::new(60.0, 10.0),
        StateKind::Simple | StateKind::Composite => {
            // Names may carry a literal `\n` (backslash-n, as written in
            // PlantUML source) — the painter's `_with-breaks` renders it
            // as a line break, so size for the widest line and the line
            // count, not the joined string. Mirrors states.typ's probe.
            let name_lines: Vec<&str> = n.display.split("\\n").collect();
            let name_cols = name_lines
                .iter()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0);
            let name_rows = name_lines.len() as f64;
            let name_w = name_cols as f64 * CHAR_W_PT + 22.0;
            if n.body.is_empty() {
                let h = (name_rows * 13.0 + 14.0).max(32.0);
                Point::new(name_w.max(56.0), h)
            } else {
                let body_w = n
                    .body
                    .iter()
                    .map(|r| r.chars().count() as f64 * BODY_CHAR_W_PT + 16.0)
                    .fold(0.0_f64, f64::max);
                let w = name_w.max(body_w).max(64.0);
                // Name band scales with the name's line count; floor at
                // the original single-line band (26pt).
                let band = (name_rows * 13.0 + 8.0).max(26.0);
                let h = band + n.body.len() as f64 * 13.0 + 8.0;
                Point::new(w, h)
            }
        }
    };
    NodeGeom { size }
}

/// Rendered `(width, height)` (pt) of a transition's label: the painter
/// measurement from pass-1 when available, else the char-count heuristic.
/// `None` when the transition carries no label. Mirrors `resolve_node_geom`.
pub(super) fn resolve_edge_label_size(
    tr: &Transition,
    ti: usize,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> Option<(f64, f64)> {
    if !has_edge_label(tr) {
        return None;
    }
    if let Some(set) = measurements {
        if let Some(m) = set.get(&state_edge_label_id(diagram_idx, ti)) {
            // A measured-but-empty label (0×0) means the probe found no
            // text; fall through to the heuristic only if that ever happens.
            if m.width_pt > 0.0 || m.height_pt > 0.0 {
                return Some((m.width_pt, m.height_pt));
            }
        }
    }
    edge_label_size(tr)
}

/// Estimate the rendered `(width, height)` (pt) of a transition's
/// `event [guard] / action` label, or `None` when it carries no label.
/// The painter renders the label at the 0.78em `_label-size` and splits
/// each part on a literal `\n`. Used both to reserve interior back-edge
/// bow room and to keep straight-edge labels inside the canvas.
fn edge_label_size(tr: &Transition) -> Option<(f64, f64)> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = tr.event.as_deref().filter(|s| !s.is_empty()) {
        parts.push(e.to_string());
    }
    if let Some(g) = tr.guard.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("[{g}]"));
    }
    if let Some(a) = tr.action.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("/ {a}"));
    }
    if parts.is_empty() {
        return None;
    }
    let joined = parts.join(" ");
    // `_with-breaks` turns a literal `\n` into a line break.
    let lines: Vec<&str> = joined.split("\\n").collect();
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let w = cols as f64 * 4.8;
    let h = lines.len() as f64 * 11.0;
    Some((w, h))
}

/// Heuristic bounding box for an anchored note's yellow sticky.
fn note_geom(body: &str) -> Point {
    let lines: Vec<&str> = if body.is_empty() {
        vec![""]
    } else {
        body.split('\n').collect()
    };
    let cols = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let w = cols as f64 * BODY_CHAR_W_PT + 16.0;
    let h = lines.len() as f64 * 13.0 + 12.0;
    Point::new(w.max(44.0), h.max(24.0))
}
