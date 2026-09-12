//! Flowchart semantics, independent of the input language and layout engine.
use super::{LayoutDirection, LineStyle, LineWeight};

#[derive(Clone, Debug, Default)]
pub struct FlowchartDiagram {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    pub subgraphs: Vec<FlowSubgraph>,
    pub direction: LayoutDirection,
    pub reverse_rank: bool,
}

#[derive(Clone, Debug)]
pub struct FlowNode {
    pub id: String,
    pub label: String,
    pub shape: FlowShape,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub arrow: bool,
    pub style: LineStyle,
    pub weight: LineWeight,
    pub label: Option<String>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct FlowSubgraph {
    pub id: String,
    pub label: String,
    pub nodes: Vec<String>,
    pub children: Vec<usize>,
    pub line: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowShape {
    Rect,
    Rounded,
    Stadium,
    Diamond,
    Circle,
    Cylinder,
    Asymmetric,
}
