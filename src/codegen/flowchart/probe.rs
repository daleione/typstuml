//! Flowchart probe identity and pass-1 emission.
use super::emit::{node_spec, plain_text};
use crate::ir::FlowchartDiagram;

pub(super) fn node_id(diagram: usize, id: &str) -> String {
    let encoded: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    format!("mf-{diagram}-{encoded}")
}
pub(super) fn subgraph_id(diagram: usize, group: usize) -> String {
    format!("mp-{diagram}-{group}")
}
pub(super) fn edge_id(diagram: usize, edge: usize) -> String {
    format!("me-{diagram}-{edge}")
}
pub(crate) fn has_probes(diag: &FlowchartDiagram) -> bool {
    !diag.nodes.is_empty() || !diag.subgraphs.is_empty()
}
pub(crate) fn collect(
    diag: &FlowchartDiagram,
    diagram: usize,
    out: &mut String,
    ids: &mut Vec<String>,
) {
    for node in &diag.nodes {
        let id = node_id(diagram, &node.id);
        out.push_str(&format!("#flowchart-probe(id: \"{id}\", spec: ("));
        node_spec(out, node);
        out.push_str("))\n");
        ids.push(id);
    }
    for (i, group) in diag.subgraphs.iter().enumerate() {
        let id = subgraph_id(diagram, i);
        out.push_str(&format!(
            "#container-probe(id: \"{id}\", label: [{}])\n",
            plain_text(&group.label)
        ));
        ids.push(id);
    }
    for (i, edge) in diag.edges.iter().enumerate() {
        let Some(label) = edge.label.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let id = edge_id(diagram, i);
        out.push_str(&format!(
            "#graph-edge-label-probe(id: \"{id}\", label: [{}])\n",
            plain_text(label)
        ));
        ids.push(id);
    }
}
