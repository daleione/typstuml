use super::ast::Chain;
use crate::diagnostics::{CompatMode, Diagnostic, Error, Result};
use crate::ir::*;
use std::collections::{HashMap, HashSet};

/// Merge only after validating the whole chain, including declarations that
/// repeat within it. This makes unsupported-shape recovery transactional.
pub fn lower(
    mut diag: FlowchartDiagram,
    chains: Vec<Chain>,
    subgraphs: &HashSet<String>,
    compat: CompatMode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<FlowchartDiagram> {
    let mut index = HashMap::<String, usize>::new();
    let mut explicit_shapes = HashMap::<String, FlowShape>::new();
    let mut owners: Vec<(HashSet<usize>, HashSet<usize>)> = Vec::new();
    for chain in chains {
        let mut pending = HashMap::new();
        let mut conflict = None;
        for node in &chain.nodes {
            if subgraphs.contains(&node.id) {
                return Err(Error::Parse {
                    line: chain.span.line,
                    message: format!(
                        "subgraph {:?} cannot also be a node or edge endpoint",
                        node.id
                    ),
                });
            }
            if let Some(shape) = node.shape {
                if let Some(old) = pending
                    .get(&node.id)
                    .or_else(|| explicit_shapes.get(&node.id))
                {
                    if *old != shape {
                        conflict = Some(node.id.clone());
                        break;
                    }
                }
                pending.insert(node.id.clone(), shape);
            }
        }
        if let Some(id) = conflict {
            let error = Error::Unsupported {
                kind: "Mermaid shape redefinition",
                detail: format!(
                    "line {}: node {id:?}; entire statement skipped",
                    chain.span.line
                ),
            };
            super::recover(error, chain.span, compat, diagnostics)?;
            continue;
        }
        explicit_shapes.extend(pending);
        let standalone = chain.edges.is_empty();
        for node in &chain.nodes {
            let i = *index.entry(node.id.clone()).or_insert_with(|| {
                let i = diag.nodes.len();
                diag.nodes.push(FlowNode {
                    id: node.id.clone(),
                    label: node.id.clone(),
                    line: chain.span.line,
                    shape: FlowShape::Rect,
                });
                owners.push((HashSet::new(), HashSet::new()));
                i
            });
            if let Some(label) = &node.label {
                diag.nodes[i].label = label.clone();
            }
            if let Some(shape) = node.shape {
                diag.nodes[i].shape = shape;
            }
            if let Some(owner) = chain.owner {
                if standalone || node.shape.is_some() {
                    owners[i].0.insert(owner);
                } else {
                    owners[i].1.insert(owner);
                }
            }
        }
        for (nodes, edge) in chain.nodes.windows(2).zip(chain.edges) {
            diag.edges.push(FlowEdge {
                from: nodes[0].id.clone(),
                to: nodes[1].id.clone(),
                arrow: edge.arrow,
                style: edge.style,
                weight: edge.weight,
                label: edge.label,
                line: chain.span.line,
            });
        }
    }
    for (entity, (explicit, implicit)) in diag.nodes.iter().zip(owners) {
        let candidates = if explicit.is_empty() {
            implicit
        } else {
            explicit
        };
        if candidates.len() > 1 {
            return Err(Error::Parse {
                line: entity.line,
                message: format!("ambiguous subgraph owner for node {:?}", entity.id),
            });
        }
        if let Some(owner) = candidates.into_iter().next() {
            diag.subgraphs[owner].nodes.push(entity.id.clone());
        }
    }
    Ok(diag)
}
