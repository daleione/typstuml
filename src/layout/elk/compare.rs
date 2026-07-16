//! Numeric comparison utilities for verifying the ELK port against
//! elkjs ground truth (`tools/elk-oracle/`).
//!
//! Two levels:
//! - [`json_semantic_diff`] — order-insensitive-for-numbers JSON
//!   equality (`12` == `12.0`), used by the model roundtrip test and
//!   as a catch-all "everything identical" assertion.
//! - [`coord_diff`] — walks two layouted [`ElkNode`] trees and reports
//!   every node/edge geometry that differs beyond a tolerance. This is
//!   the milestone report: layer parity (E4) and order parity (E5)
//!   read positions off it, E6/E7 assert it comes back empty.

use serde_json::Value;

use super::graph::{ElkEdge, ElkNode};

/// Recursively compare two JSON values; numbers compare numerically
/// with tolerance `eps` (so `12` == `12.0`, and small float drift can
/// be admitted where a milestone allows it). Returns every difference
/// as a `(json-path, description)` pair; empty means equal.
pub fn json_semantic_diff(a: &Value, b: &Value, eps: f64) -> Vec<(String, String)> {
    let mut out = Vec::new();
    diff_value(a, b, eps, "$", &mut out);
    out
}

fn diff_value(a: &Value, b: &Value, eps: f64, path: &str, out: &mut Vec<(String, String)>) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN));
            if !((x - y).abs() <= eps || (x.is_nan() && y.is_nan())) {
                out.push((path.to_string(), format!("{x} != {y}")));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            for (k, va) in x {
                match y.get(k) {
                    Some(vb) => diff_value(va, vb, eps, &format!("{path}.{k}"), out),
                    None => out.push((format!("{path}.{k}"), "missing on right".into())),
                }
            }
            for k in y.keys() {
                if !x.contains_key(k) {
                    out.push((format!("{path}.{k}"), "missing on left".into()));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push((path.to_string(), format!("array len {} != {}", x.len(), y.len())));
            }
            for (i, (va, vb)) in x.iter().zip(y.iter()).enumerate() {
                diff_value(va, vb, eps, &format!("{path}[{i}]"), out);
            }
        }
        _ => {
            if a != b {
                out.push((path.to_string(), format!("{a} != {b}")));
            }
        }
    }
}

/// One geometry discrepancy between an expected (oracle) and actual
/// (Rust port) layout.
#[derive(Debug, Clone, PartialEq)]
pub struct CoordDiff {
    /// Node or edge id.
    pub id: String,
    /// Which quantity differs: `x`, `y`, `width`, `height`,
    /// `section[i].startPoint.x`, …
    pub field: String,
    pub expected: f64,
    pub actual: f64,
}

impl std::fmt::Display for CoordDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}: expected {} got {} (Δ {:+.3})",
            self.id,
            self.field,
            self.expected,
            self.actual,
            self.actual - self.expected
        )
    }
}

/// Compare two layouted graphs node-by-node (matched by id, recursing
/// through children) and edge-by-edge. Reports geometry differences
/// beyond `tol` plus structural mismatches (a missing id is reported
/// with `expected`/`actual` set to NaN).
pub fn coord_diff(expected: &ElkNode, actual: &ElkNode, tol: f64) -> Vec<CoordDiff> {
    let mut out = Vec::new();
    diff_node(expected, actual, tol, &mut out);
    out
}

fn opt_field(
    id: &str,
    field: &str,
    e: Option<f64>,
    a: Option<f64>,
    tol: f64,
    out: &mut Vec<CoordDiff>,
) {
    let (e, a) = (e.unwrap_or(0.0), a.unwrap_or(0.0));
    if (e - a).abs() > tol {
        out.push(CoordDiff { id: id.into(), field: field.into(), expected: e, actual: a });
    }
}

fn diff_node(e: &ElkNode, a: &ElkNode, tol: f64, out: &mut Vec<CoordDiff>) {
    opt_field(&e.id, "x", e.x, a.x, tol, out);
    opt_field(&e.id, "y", e.y, a.y, tol, out);
    opt_field(&e.id, "width", e.width, a.width, tol, out);
    opt_field(&e.id, "height", e.height, a.height, tol, out);

    let empty_n: Vec<ElkNode> = Vec::new();
    let e_children = e.children.as_ref().unwrap_or(&empty_n);
    let a_children = a.children.as_ref().unwrap_or(&empty_n);
    for ec in e_children {
        match a_children.iter().find(|c| c.id == ec.id) {
            Some(ac) => diff_node(ec, ac, tol, out),
            None => out.push(CoordDiff {
                id: ec.id.clone(),
                field: "missing node in actual".into(),
                expected: f64::NAN,
                actual: f64::NAN,
            }),
        }
    }
    for ac in a_children {
        if !e_children.iter().any(|c| c.id == ac.id) {
            out.push(CoordDiff {
                id: ac.id.clone(),
                field: "unexpected node in actual".into(),
                expected: f64::NAN,
                actual: f64::NAN,
            });
        }
    }

    let empty_e: Vec<ElkEdge> = Vec::new();
    let e_edges = e.edges.as_ref().unwrap_or(&empty_e);
    let a_edges = a.edges.as_ref().unwrap_or(&empty_e);
    for ee in e_edges {
        match a_edges.iter().find(|x| x.id == ee.id) {
            Some(ae) => diff_edge(ee, ae, tol, out),
            None => out.push(CoordDiff {
                id: ee.id.clone(),
                field: "missing edge in actual".into(),
                expected: f64::NAN,
                actual: f64::NAN,
            }),
        }
    }
}

fn diff_edge(e: &ElkEdge, a: &ElkEdge, tol: f64, out: &mut Vec<CoordDiff>) {
    // Edge labels: matched by index (ELK preserves input label order).
    let empty_l = Vec::new();
    let el = e.labels.as_ref().unwrap_or(&empty_l);
    let al = a.labels.as_ref().unwrap_or(&empty_l);
    if el.len() != al.len() {
        out.push(CoordDiff {
            id: e.id.clone(),
            field: format!("label count {} != {}", el.len(), al.len()),
            expected: el.len() as f64,
            actual: al.len() as f64,
        });
    }
    for (i, (le, la)) in el.iter().zip(al.iter()).enumerate() {
        opt_field(&e.id, &format!("label[{i}].x"), le.x, la.x, tol, out);
        opt_field(&e.id, &format!("label[{i}].y"), le.y, la.y, tol, out);
        opt_field(&e.id, &format!("label[{i}].width"), le.width, la.width, tol, out);
        opt_field(&e.id, &format!("label[{i}].height"), le.height, la.height, tol, out);
    }

    let empty = Vec::new();
    let es = e.sections.as_ref().unwrap_or(&empty);
    let asx = a.sections.as_ref().unwrap_or(&empty);
    if es.len() != asx.len() {
        out.push(CoordDiff {
            id: e.id.clone(),
            field: format!("section count {} != {}", es.len(), asx.len()),
            expected: es.len() as f64,
            actual: asx.len() as f64,
        });
    }
    for (i, (se, sa)) in es.iter().zip(asx.iter()).enumerate() {
        let pts = |s: &super::graph::ElkEdgeSection| {
            let mut v = vec![(s.start_point.x, s.start_point.y)];
            if let Some(b) = &s.bend_points {
                v.extend(b.iter().map(|p| (p.x, p.y)));
            }
            v.push((s.end_point.x, s.end_point.y));
            v
        };
        let (pe, pa) = (pts(se), pts(sa));
        if pe.len() != pa.len() {
            out.push(CoordDiff {
                id: e.id.clone(),
                field: format!("section[{i}] point count {} != {}", pe.len(), pa.len()),
                expected: pe.len() as f64,
                actual: pa.len() as f64,
            });
            continue;
        }
        for (j, ((ex, ey), (ax, ay))) in pe.iter().zip(pa.iter()).enumerate() {
            if (ex - ax).abs() > tol {
                out.push(CoordDiff {
                    id: e.id.clone(),
                    field: format!("section[{i}].pt[{j}].x"),
                    expected: *ex,
                    actual: *ax,
                });
            }
            if (ey - ay).abs() > tol {
                out.push(CoordDiff {
                    id: e.id.clone(),
                    field: format!("section[{i}].pt[{j}].y"),
                    expected: *ey,
                    actual: *ay,
                });
            }
        }
    }
}
