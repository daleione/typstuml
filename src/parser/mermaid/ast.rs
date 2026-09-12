use super::lexer::SourceSpan;
use crate::ir::{FlowShape, LineStyle, LineWeight};

#[derive(Debug)]
pub struct NodeRef {
    pub id: String,
    pub label: Option<String>,
    pub shape: Option<FlowShape>,
}
#[derive(Debug)]
pub struct EdgeSpec {
    pub arrow: bool,
    pub style: LineStyle,
    pub weight: LineWeight,
    pub label: Option<String>,
}
#[derive(Debug)]
pub struct Chain {
    pub nodes: Vec<NodeRef>,
    pub edges: Vec<EdgeSpec>,
    pub owner: Option<usize>,
    pub span: SourceSpan,
}
