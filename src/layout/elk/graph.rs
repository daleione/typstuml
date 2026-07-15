//! The ELK JSON graph model — the exact structure elkjs consumes as
//! layout input and returns as layout output (see the "ELK JSON
//! format" docs and `tools/elk-oracle/golden/*.stages.json` for live
//! examples: `pass1Input`/`pass2Input` are inputs, `pass1Output`/
//! `pass2Output` are outputs).
//!
//! Every struct carries a `#[serde(flatten)] extra` map so fields this
//! model doesn't know about survive a deserialize→serialize roundtrip
//! unchanged — the roundtrip test in `tests/elk_port.rs` is what
//! proves the model covers (or at least preserves) the whole schema
//! the oracle emits.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A node — the root graph itself, a package/group, or a leaf shape.
/// Coordinates are relative to the parent's origin, ELK convention.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ElkNode {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Layout options, e.g. `"elk.algorithm": "layered"`. Values are
    /// kept as raw JSON: elkjs accepts strings, numbers and booleans.
    #[serde(rename = "layoutOptions", skip_serializing_if = "Option::is_none")]
    pub layout_options: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<ElkNode>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<ElkPort>>,
    /// Edges attach to the node that *contains* both endpoints (their
    /// LCA) — for `hierarchyHandling: INCLUDE_CHILDREN` graphs edges
    /// appear at every level of the tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edges: Option<Vec<ElkEdge>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<ElkLabel>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ElkPort {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(rename = "layoutOptions", skip_serializing_if = "Option::is_none")]
    pub layout_options: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<ElkLabel>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ElkEdge {
    pub id: String,
    /// Source node or port ids (ELK JSON allows several; draw-uml
    /// always emits exactly one).
    pub sources: Vec<String>,
    pub targets: Vec<String>,
    #[serde(rename = "layoutOptions", skip_serializing_if = "Option::is_none")]
    pub layout_options: Option<Map<String, Value>>,
    /// Output only: the routed polyline, one section per edge for
    /// non-hyperedges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<ElkEdgeSection>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<ElkLabel>>,
    /// Output only: id of the node whose coordinate system the
    /// sections are expressed in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(rename = "junctionPoints", skip_serializing_if = "Option::is_none")]
    pub junction_points: Option<Vec<ElkPoint>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ElkEdgeSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "startPoint")]
    pub start_point: ElkPoint,
    #[serde(rename = "endPoint")]
    pub end_point: ElkPoint,
    #[serde(rename = "bendPoints", skip_serializing_if = "Option::is_none")]
    pub bend_points: Option<Vec<ElkPoint>>,
    #[serde(rename = "incomingShape", skip_serializing_if = "Option::is_none")]
    pub incoming_shape: Option<String>,
    #[serde(rename = "outgoingShape", skip_serializing_if = "Option::is_none")]
    pub outgoing_shape: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct ElkPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ElkLabel {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(rename = "layoutOptions", skip_serializing_if = "Option::is_none")]
    pub layout_options: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ElkNode {
    /// Depth-first walk over `self` and every descendant node.
    pub fn walk(&self, f: &mut impl FnMut(&ElkNode)) {
        f(self);
        if let Some(children) = &self.children {
            for c in children {
                c.walk(f);
            }
        }
    }

    /// Find a descendant (or self) by id.
    pub fn find(&self, id: &str) -> Option<&ElkNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.as_deref()?.iter().find_map(|c| c.find(id))
    }
}
