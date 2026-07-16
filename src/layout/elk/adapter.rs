//! The draw-uml ELK adapter layer, reimplemented for TypstUML: builds the
//! two ELK layout passes from a *measured* diagram model, extracts the
//! laid-out result, and applies the post-processing passes.
//!
//! Behavioral reference: `@markdown-viewer/draw-uml` 1.4.0
//! (`src/layout/elk/elk-adapter.ts`, `elk-extractor.ts`,
//! `src/layout/post-process.ts`; GPL-3.0-only / commercial dual license —
//! this module is a scoped behavioral reimplementation kept isolated
//! here; see docs/elk-port-plan.md for the licensing note). Verified
//! stage-by-stage against `tools/elk-oracle/golden/*.stages.json`.
//!
//! Scope (matching the cuca desc-flavor architecture diagrams TypstUML
//! feeds it): DOWN direction, plain node-to-node edges (optional center /
//! tail / head labels), flat or nested packages, icon nodes with an
//! outside bottom label (lollipop interfaces), diagram title. Notes,
//! legends, swimlanes, concurrent regions, field ports and state
//! start/end markers are outside the scope and rejected loudly where
//! cheap.
//!
//! Sizes are inputs (plan §3): every node/label/title extent comes from
//! the caller's measurement (Typst probe in production, the golden
//! `pass1Input` in tests).

use std::collections::{BTreeMap, HashMap};

use serde_json::{Map, Value};

use super::alg::graph::LGraphArena;
use super::alg::hierarchical;
use super::alg::transform;
use super::graph as json;

// ----------------------------------------------------------------------
// Input model
// ----------------------------------------------------------------------

/// Theme-derived spacing numbers (draw-uml `elkSpacing`).
#[derive(Debug, Clone, Copy)]
pub struct AdapterSpacing {
    /// `theme.nodeGap` — edgeNode / selfLoop / componentComponent, and
    /// nodeNode inside groups.
    pub node_gap: f64,
    /// `theme.layerGap` — nodeNodeBetweenLayers.
    pub layer_gap: f64,
    /// `theme.contentPad` — edgeEdge (+ betweenLayers) and the root-level
    /// nodeNode when groups are present.
    pub content_pad: f64,
    /// `theme.edgeGap` — edge label spacing and the post-process minimum
    /// edge gap.
    pub edge_gap: f64,
}

impl Default for AdapterSpacing {
    fn default() -> Self {
        // draw-uml theme defaults at fontSize 14 (the benchmark's values).
        Self { node_gap: 20.0, layer_gap: 40.0, content_pad: 10.0, edge_gap: 5.0 }
    }
}

/// A measured leaf node.
#[derive(Debug, Clone)]
pub struct AdapterNode {
    pub id: String,
    /// Full box (icon + outside label for graphic nodes).
    pub width: f64,
    pub height: f64,
    /// Icon-only extent for lollipop/icon nodes (`Renderer.graphicSize`).
    /// The ELK node shrinks to this and the rest becomes an outside label.
    pub graphic: Option<(f64, f64)>,
    /// The node's label text (only used for graphic nodes' outside label).
    pub node_label: Option<String>,
}

/// A group (package) with measured padding.
#[derive(Debug, Clone)]
pub struct AdapterGroup {
    pub id: String,
    /// Child ids in order — leaf nodes or nested groups.
    pub children: Vec<String>,
    /// User-frame padding `[top,left,bottom,right]`.
    pub padding: (f64, f64, f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeLabelPlacement {
    Center,
    Tail,
    Head,
}

#[derive(Debug, Clone)]
pub struct AdapterEdgeLabel {
    pub text: String,
    pub width: f64,
    pub height: f64,
    pub placement: EdgeLabelPlacement,
}

#[derive(Debug, Clone)]
pub struct AdapterEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    /// Measured labels (center / tail / head).
    pub labels: Vec<AdapterEdgeLabel>,
    /// `direction === 'left' | 'up'` — endpoints swapped for layout and
    /// waypoints reversed on extraction.
    pub inverted: bool,
}

/// The measured diagram model the adapter consumes.
#[derive(Debug, Clone, Default)]
pub struct AdapterModel {
    pub nodes: Vec<AdapterNode>,
    pub groups: Vec<AdapterGroup>,
    pub edges: Vec<AdapterEdge>,
    pub spacing: AdapterSpacing,
    /// Measured title extent, if the diagram has a title.
    pub title: Option<(f64, f64)>,
}

// ----------------------------------------------------------------------
// Pass builders
// ----------------------------------------------------------------------

fn fmt(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn opts(pairs: &[(&str, String)]) -> Option<Map<String, Value>> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_string(), Value::String(v.clone()));
    }
    Some(m)
}

impl AdapterModel {
    fn group_map(&self) -> HashMap<&str, &AdapterGroup> {
        self.groups.iter().map(|g| (g.id.as_str(), g)).collect()
    }

    fn node_map(&self) -> HashMap<&str, &AdapterNode> {
        self.nodes.iter().map(|n| (n.id.as_str(), n)).collect()
    }

    /// Ids that are children of some group.
    fn grouped_ids(&self) -> HashMap<&str, &str> {
        let mut m = HashMap::new();
        for g in &self.groups {
            for c in &g.children {
                m.insert(c.as_str(), g.id.as_str());
            }
        }
        m
    }

    /// Root order: groupless nodes in model order, then top-level groups
    /// in model order (draw-uml's renderer tree).
    fn root_children_ids(&self) -> Vec<&str> {
        let grouped = self.grouped_ids();
        let mut out: Vec<&str> = Vec::new();
        for n in &self.nodes {
            if !grouped.contains_key(n.id.as_str()) {
                out.push(&n.id);
            }
        }
        for g in &self.groups {
            if !grouped.contains_key(g.id.as_str()) {
                out.push(&g.id);
            }
        }
        out
    }
}

/// Map a leaf node, applying the icon shrink: graphic nodes become
/// icon-sized with an `OUTSIDE V_BOTTOM H_CENTER` label covering the rest
/// of the measured box.
fn map_leaf(n: &AdapterNode) -> json::ElkNode {
    let mut elk = json::ElkNode { id: n.id.clone(), ..Default::default() };
    if let Some((gw, gh)) = n.graphic {
        elk.width = Some(gw);
        elk.height = Some(gh);
        if let Some(label) = &n.node_label {
            elk.labels = Some(vec![json::ElkLabel {
                text: label.clone(),
                width: Some(n.width),
                height: Some((n.height - gh).max(0.0)),
                layout_options: opts(&[(
                    "elk.nodeLabels.placement",
                    "OUTSIDE V_BOTTOM H_CENTER".into(),
                )]),
                ..Default::default()
            }]);
        }
    } else {
        elk.width = Some(n.width);
        elk.height = Some(n.height);
    }
    elk
}

/// Recursively map a group and its children. `full` adds the pass-2
/// per-group spacing overrides on top of the padding.
fn map_group(
    model: &AdapterModel,
    g: &AdapterGroup,
    full: bool,
) -> json::ElkNode {
    let node_map = model.node_map();
    let group_map = model.group_map();
    let children: Vec<json::ElkNode> = g
        .children
        .iter()
        .map(|c| {
            if let Some(child_group) = group_map.get(c.as_str()) {
                map_group(model, child_group, full)
            } else {
                map_leaf(node_map[c.as_str()])
            }
        })
        .collect();

    let (top, left, bottom, right) = g.padding;
    let padding = format!("[top={},left={},bottom={},right={}]", fmt(top), fmt(left), fmt(bottom), fmt(right));
    let es = model.spacing;
    let layout_options = if full {
        opts(&[
            ("elk.padding", padding),
            ("elk.spacing.nodeNode", fmt(es.node_gap)),
            ("elk.layered.spacing.nodeNodeBetweenLayers", fmt(es.layer_gap)),
            ("elk.spacing.edgeNode", fmt(es.node_gap)),
            ("elk.spacing.edgeEdge", fmt(es.content_pad)),
            ("elk.spacing.nodeSelfLoop", fmt(es.node_gap)),
            ("elk.layered.spacing.edgeEdgeBetweenLayers", fmt(es.content_pad)),
            ("elk.layered.spacing.edgeNodeBetweenLayers", fmt(es.node_gap)),
        ])
    } else {
        opts(&[("elk.padding", padding)])
    };

    json::ElkNode {
        id: g.id.clone(),
        children: Some(children),
        layout_options,
        ..Default::default()
    }
}

/// Common root scaffolding for both passes.
fn build_root(model: &AdapterModel, full: bool) -> json::ElkNode {
    let node_map = model.node_map();
    let group_map = model.group_map();
    let children: Vec<json::ElkNode> = model
        .root_children_ids()
        .into_iter()
        .map(|id| {
            if let Some(g) = group_map.get(id) {
                map_group(model, g, full)
            } else {
                map_leaf(node_map[id])
            }
        })
        .collect();

    let es = model.spacing;
    let has_groups = !model.groups.is_empty();
    // Root-level nodeNode: reduced (contentPad) when groups are present —
    // groups carry their own padding.
    let root_node_node = if has_groups { es.content_pad } else { es.node_gap };
    let root_between_layers = es.layer_gap;

    let mut pairs: Vec<(&str, String)> = vec![
        ("elk.algorithm", "layered".into()),
        ("elk.direction", "DOWN".into()),
        ("elk.edgeRouting", "ORTHOGONAL".into()),
        ("elk.spacing.nodeNode", fmt(root_node_node)),
        ("elk.layered.spacing.nodeNodeBetweenLayers", fmt(root_between_layers)),
        ("elk.spacing.edgeNode", fmt(es.node_gap)),
        ("elk.spacing.edgeEdge", fmt(es.content_pad)),
        ("elk.spacing.nodeSelfLoop", fmt(es.node_gap)),
        ("elk.spacing.componentComponent", fmt(es.node_gap)),
        ("elk.layered.spacing.edgeEdgeBetweenLayers", fmt(es.content_pad)),
        ("elk.layered.spacing.edgeNodeBetweenLayers", fmt(es.node_gap)),
    ];
    if full {
        pairs.push(("elk.spacing.edgeLabel", fmt(es.edge_gap)));
        pairs.push(("elk.layered.nodePlacement.bk.fixedAlignment", "BALANCED".into()));
        pairs.push(("elk.contentAlignment", "H_CENTER V_CENTER".into()));
        pairs.push(("elk.layered.considerModelOrder.strategy", "NODES_AND_EDGES".into()));
        pairs.push(("elk.layered.highDegreeNodes.treatment", "true".into()));
        pairs.push(("elk.layered.highDegreeNodes.threshold", "8".into()));
        pairs.push(("elk.layered.mergeEdges", "false".into()));
    } else {
        pairs.push(("elk.layered.considerModelOrder.strategy", "NODES_AND_EDGES".into()));
        pairs.push(("elk.layered.compaction.postCompaction.strategy", "LEFT".into()));
    }
    if has_groups {
        pairs.push(("elk.hierarchyHandling", "INCLUDE_CHILDREN".into()));
    }

    json::ElkNode {
        id: "root".into(),
        layout_options: opts(&pairs),
        children: Some(children),
        edges: Some(Vec::new()),
        ..Default::default()
    }
}

/// Collect edges in model order with layout endpoints (inversion applied)
/// and measured labels. `simple` mirrors draw-uml's pass-1 mapping, which
/// keeps the labels but drops every edge layout option (notably the
/// `edgeLabels.inline` marker — an elkjs no-op either way, since the
/// option only acts when set on a label element).
fn collect_edges(model: &AdapterModel, simple: bool) -> Vec<json::ElkEdge> {
    model
        .edges
        .iter()
        .map(|e| {
            let (from, to) =
                if e.inverted { (e.to.clone(), e.from.clone()) } else { (e.from.clone(), e.to.clone()) };
            let labels: Vec<json::ElkLabel> = e
                .labels
                .iter()
                .map(|l| {
                    // draw-uml stamps its own raw `placement` field on every
                    // label and adds the real ELK option for tail/head only.
                    let placement_str = match l.placement {
                        EdgeLabelPlacement::Center => "center",
                        EdgeLabelPlacement::Tail => "tail",
                        EdgeLabelPlacement::Head => "head",
                    };
                    let mut extra = Map::new();
                    extra.insert("placement".into(), Value::String(placement_str.into()));
                    json::ElkLabel {
                        text: l.text.clone(),
                        width: Some(l.width),
                        height: Some(l.height),
                        layout_options: match l.placement {
                            EdgeLabelPlacement::Center => None,
                            EdgeLabelPlacement::Tail => opts(&[(
                                "org.eclipse.elk.edgeLabels.placement",
                                "TAIL".into(),
                            )]),
                            EdgeLabelPlacement::Head => opts(&[(
                                "org.eclipse.elk.edgeLabels.placement",
                                "HEAD".into(),
                            )]),
                        },
                        extra,
                        ..Default::default()
                    }
                })
                .collect();
            let has_labels = !labels.is_empty();
            json::ElkEdge {
                id: e.id.clone(),
                sources: vec![from],
                targets: vec![to],
                labels: if has_labels { Some(labels) } else { None },
                layout_options: if has_labels && !simple {
                    opts(&[("org.eclipse.elk.edgeLabels.inline", "true".into())])
                } else {
                    None
                },
                ..Default::default()
            }
        })
        .collect()
}

/// Ancestor paths (root → immediate parent) for every node/group id.
fn ancestor_map(root: &json::ElkNode) -> HashMap<String, Vec<String>> {
    let mut map = HashMap::new();
    fn walk(node: &json::ElkNode, ancestors: &[String], map: &mut HashMap<String, Vec<String>>) {
        map.insert(node.id.clone(), ancestors.to_vec());
        if let Some(children) = &node.children {
            let mut child_anc = ancestors.to_vec();
            child_anc.push(node.id.clone());
            for c in children {
                walk(c, &child_anc, map);
            }
        }
    }
    for c in root.children.as_deref().unwrap_or(&[]) {
        walk(c, &[root.id.clone()], &mut map);
    }
    map
}

/// Ancestor↔descendant group edges get a 1×1 proxy child inside the
/// ancestor, so both endpoints become siblings.
fn add_compound_edge_proxies(
    root: &mut json::ElkNode,
    edges: &mut [json::ElkEdge],
    model: &AdapterModel,
) {
    let group_map = model.group_map();
    let grouped = model.grouped_ids();
    let is_ancestor = |ancestor: &str, descendant: &str| -> bool {
        let mut cur = descendant;
        while let Some(&parent) = grouped.get(cur) {
            if parent == ancestor {
                return true;
            }
            cur = parent;
        }
        false
    };
    fn find_mut<'a>(node: &'a mut json::ElkNode, id: &str) -> Option<&'a mut json::ElkNode> {
        if node.id == id {
            return Some(node);
        }
        for c in node.children.as_mut()?.iter_mut() {
            if let Some(found) = find_mut(c, id) {
                return Some(found);
            }
        }
        None
    }
    for edge in edges.iter_mut() {
        let src = edge.sources[0].clone();
        let tgt = edge.targets[0].clone();
        if !group_map.contains_key(src.as_str()) || !group_map.contains_key(tgt.as_str()) {
            continue;
        }
        let (ancestor, is_source) = if is_ancestor(&src, &tgt) {
            (src.clone(), true)
        } else if is_ancestor(&tgt, &src) {
            (tgt.clone(), false)
        } else {
            continue;
        };
        let proxy_id = format!("__proxy_{ancestor}");
        if let Some(node) = find_mut(root, &ancestor) {
            let children = node.children.get_or_insert_with(Vec::new);
            if !children.iter().any(|c| c.id == proxy_id) {
                children.push(json::ElkNode {
                    id: proxy_id.clone(),
                    width: Some(1.0),
                    height: Some(1.0),
                    ..Default::default()
                });
            }
            if is_source {
                edge.sources = vec![proxy_id];
            } else {
                edge.targets = vec![proxy_id];
            }
        }
    }
}

/// Place each edge at its endpoints' lowest common ancestor container.
fn distribute_edges(root: &mut json::ElkNode, edges: Vec<json::ElkEdge>) {
    let anc = ancestor_map(root);
    let mut per_container: HashMap<String, Vec<json::ElkEdge>> = HashMap::new();
    for edge in edges {
        let sa = anc.get(&edge.sources[0]);
        let ta = anc.get(&edge.targets[0]);
        let mut lca = "root".to_string();
        if let (Some(pa), Some(pb)) = (sa, ta) {
            for (a, b) in pa.iter().zip(pb.iter()) {
                if a == b {
                    lca = a.clone();
                } else {
                    break;
                }
            }
        }
        per_container.entry(lca).or_default().push(edge);
    }
    fn assign(node: &mut json::ElkNode, per: &mut HashMap<String, Vec<json::ElkEdge>>) {
        if let Some(edges) = per.remove(&node.id) {
            node.edges.get_or_insert_with(Vec::new).extend(edges);
        }
        if let Some(children) = node.children.as_mut() {
            for c in children {
                assign(c, per);
            }
        }
    }
    assign(root, &mut per_container);
}

/// Build the pass-1 ("simple", port-free) ELK graph.
pub fn to_elk_simple(model: &AdapterModel) -> json::ElkNode {
    let mut root = build_root(model, false);
    let edges = collect_edges(model, true);
    // Pass 1 strips label-free port suffixes — no ports in scope, and
    // labels ride along unchanged.
    let mut edges: Vec<json::ElkEdge> = edges;
    add_compound_edge_proxies(&mut root, &mut edges, model);
    distribute_edges(&mut root, edges);
    root
}

/// Build the pass-2 (full options) ELK graph. `positions` — pass-1
/// absolute node centers — only influences field-port side selection,
/// which is outside the current scope; accepted for interface parity.
pub fn to_elk(model: &AdapterModel, _positions: &HashMap<String, (f64, f64)>) -> json::ElkNode {
    let mut root = build_root(model, true);
    let mut edges = collect_edges(model, false);
    add_compound_edge_proxies(&mut root, &mut edges, model);
    distribute_edges(&mut root, edges);
    root
}

/// Absolute node centers from a laid-out ELK tree (pass-1 → pass-2
/// handshake).
pub fn collect_node_positions(root: &json::ElkNode) -> HashMap<String, (f64, f64)> {
    let mut out = HashMap::new();
    fn walk(node: &json::ElkNode, px: f64, py: f64, out: &mut HashMap<String, (f64, f64)>) {
        for child in node.children.as_deref().unwrap_or(&[]) {
            let ax = px + child.x.unwrap_or(0.0);
            let ay = py + child.y.unwrap_or(0.0);
            let w = child.width.unwrap_or(0.0);
            let h = child.height.unwrap_or(0.0);
            out.insert(child.id.clone(), (ax + w / 2.0, ay + h / 2.0));
            walk(child, ax, ay, out);
        }
    }
    walk(root, 0.0, 0.0, &mut out);
    out
}

// ----------------------------------------------------------------------
// Extraction
// ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutNode {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Center of the outside label, when the node has one.
    pub xlabel_pos: Option<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub points: Vec<(f64, f64)>,
    pub label_pos: Option<(f64, f64)>,
    pub label_size: Option<(f64, f64)>,
    pub card_from_pos: Option<(f64, f64)>,
    pub card_to_pos: Option<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutGroup {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The adapter's layout result (draw-uml `LayoutResult` subset).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub nodes: BTreeMap<String, LayoutNode>,
    pub edges: Vec<LayoutEdge>,
    pub groups: BTreeMap<String, LayoutGroup>,
}

/// Extract absolute node/group boxes and edge waypoints from the
/// laid-out pass-2 tree, then simplify minor orthogonal bends.
pub fn extract(result: &json::ElkNode, model: &AdapterModel) -> Layout {
    let node_map = model.node_map();
    let mut layout = Layout::default();

    // Nodes & groups (relative → absolute).
    fn collect(
        elk: &json::ElkNode,
        px: f64,
        py: f64,
        node_map: &HashMap<&str, &AdapterNode>,
        layout: &mut Layout,
    ) {
        for child in elk.children.as_deref().unwrap_or(&[]) {
            let ax = px + child.x.unwrap_or(0.0);
            let ay = py + child.y.unwrap_or(0.0);
            let w = child.width.unwrap_or(0.0);
            let h = child.height.unwrap_or(0.0);
            if child.children.as_ref().is_some_and(|c| !c.is_empty()) {
                layout.groups.insert(
                    child.id.clone(),
                    LayoutGroup { id: child.id.clone(), x: ax, y: ay, width: w, height: h },
                );
                collect(child, ax, ay, node_map, layout);
            } else {
                if child.id.starts_with("__proxy_") {
                    continue;
                }
                let known = node_map.get(child.id.as_str());
                let (node_w, node_h) = match known {
                    Some(n) => (n.width, n.height),
                    None => (w, h),
                };
                let mut x = ax;
                let y = ay;
                if let Some(n) = known {
                    if let Some((gw, _gh)) = n.graphic {
                        x -= (n.width - gw) / 2.0;
                    }
                }
                let mut ln = LayoutNode {
                    id: child.id.clone(),
                    x,
                    y,
                    width: node_w,
                    height: node_h,
                    xlabel_pos: None,
                };
                if let Some(labels) = &child.labels {
                    if let Some(l) = labels.first() {
                        if let (Some(lx), Some(ly)) = (l.x, l.y) {
                            ln.xlabel_pos = Some((
                                ax + lx + l.width.unwrap_or(0.0) / 2.0,
                                ay + ly + l.height.unwrap_or(0.0) / 2.0,
                            ));
                        }
                    }
                }
                layout.nodes.insert(child.id.clone(), ln);
            }
        }
    }
    collect(result, 0.0, 0.0, &node_map, &mut layout);

    // Container absolute offsets for edge coordinates.
    let mut offsets: HashMap<String, (f64, f64)> = HashMap::new();
    fn offsets_walk(node: &json::ElkNode, px: f64, py: f64, out: &mut HashMap<String, (f64, f64)>) {
        let ax = px + node.x.unwrap_or(0.0);
        let ay = py + node.y.unwrap_or(0.0);
        out.insert(node.id.clone(), (ax, ay));
        for c in node.children.as_deref().unwrap_or(&[]) {
            offsets_walk(c, ax, ay, out);
        }
    }
    offsets_walk(result, 0.0, 0.0, &mut offsets);

    // Edges from every container, in tree order.
    let mut collected: Vec<(json::ElkEdge, String)> = Vec::new();
    fn edges_walk(node: &json::ElkNode, out: &mut Vec<(json::ElkEdge, String)>) {
        for e in node.edges.as_deref().unwrap_or(&[]) {
            out.push((e.clone(), node.id.clone()));
        }
        for c in node.children.as_deref().unwrap_or(&[]) {
            edges_walk(c, out);
        }
    }
    edges_walk(result, &mut collected);

    let model_edges: HashMap<&str, &AdapterEdge> =
        model.edges.iter().map(|e| (e.id.as_str(), e)).collect();

    for (elk_edge, container) in collected {
        let Some(sem) = model_edges.get(elk_edge.id.as_str()) else { continue };
        let (ox, oy) = offsets.get(&container).copied().unwrap_or((0.0, 0.0));
        let mut points: Vec<(f64, f64)> = Vec::new();
        for section in elk_edge.sections.as_deref().unwrap_or(&[]) {
            points.push((section.start_point.x + ox, section.start_point.y + oy));
            for bp in section.bend_points.as_deref().unwrap_or(&[]) {
                points.push((bp.x + ox, bp.y + oy));
            }
            points.push((section.end_point.x + ox, section.end_point.y + oy));
        }
        if sem.inverted && points.len() > 1 {
            points.reverse();
        }

        let mut label_pos = None;
        let mut label_size = None;
        let mut card_from_pos = None;
        let mut card_to_pos = None;
        if let Some(labels) = &elk_edge.labels {
            for (i, lbl) in labels.iter().enumerate() {
                let (Some(lx), Some(ly)) = (lbl.x, lbl.y) else { continue };
                let center = (
                    ox + lx + lbl.width.unwrap_or(0.0) / 2.0,
                    oy + ly + lbl.height.unwrap_or(0.0) / 2.0,
                );
                match sem.labels.get(i).map(|l| l.placement) {
                    Some(EdgeLabelPlacement::Tail) => card_from_pos = Some(center),
                    Some(EdgeLabelPlacement::Head) => card_to_pos = Some(center),
                    _ => {
                        if label_pos.is_none() {
                            label_pos = Some(center);
                            label_size = Some((
                                lbl.width.unwrap_or(0.0).ceil(),
                                lbl.height.unwrap_or(0.0).ceil(),
                            ));
                        }
                    }
                }
            }
        }

        let mut points = points;
        if points.len() > 2 {
            points = simplify_orthogonal_edge(&points);
        }
        layout.edges.push(LayoutEdge {
            id: sem.id.clone(),
            from: sem.from.clone(),
            to: sem.to.clone(),
            points,
            label_pos,
            label_size,
            card_from_pos,
            card_to_pos,
        });
    }

    layout
}

/// Two-pass orthogonal simplification: drop collinear midpoints, then
/// merge sub-threshold Z-bends onto a midline.
fn simplify_orthogonal_edge(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    const BEND_THRESHOLD: f64 = 5.0;
    const COL_TOL: f64 = 1.0;

    let mut cleaned: Vec<(f64, f64)> = vec![points[0]];
    for i in 1..points.len() - 1 {
        let prev = *cleaned.last().unwrap();
        let cur = points[i];
        let next = points[i + 1];
        let collinear_x = (prev.0 - cur.0).abs() <= COL_TOL && (cur.0 - next.0).abs() <= COL_TOL;
        let collinear_y = (prev.1 - cur.1).abs() <= COL_TOL && (cur.1 - next.1).abs() <= COL_TOL;
        if !collinear_x && !collinear_y {
            cleaned.push(cur);
        }
    }
    cleaned.push(points[points.len() - 1]);

    if cleaned.len() < 4 {
        return cleaned;
    }
    let mut result: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < cleaned.len() {
        if i + 3 < cleaned.len() {
            let (a, b, c, d) = (cleaned[i], cleaned[i + 1], cleaned[i + 2], cleaned[i + 3]);
            // V-H-V with a small horizontal jog.
            if a.0 == b.0 && b.1 == c.1 && c.0 == d.0 && a.0 != d.0 && (a.0 - d.0).abs() <= BEND_THRESHOLD {
                let mid_x = (a.0 + d.0) / 2.0;
                result.push((mid_x, a.1));
                result.push((mid_x, d.1));
                i += 4;
                continue;
            }
            // H-V-H with a small vertical jog.
            if a.1 == b.1 && b.0 == c.0 && c.1 == d.1 && a.1 != d.1 && (a.1 - d.1).abs() <= BEND_THRESHOLD {
                let mid_y = (a.1 + d.1) / 2.0;
                result.push((a.0, mid_y));
                result.push((d.0, mid_y));
                i += 4;
                continue;
            }
        }
        result.push(cleaned[i]);
        i += 1;
    }
    result
}

// ----------------------------------------------------------------------
// Post-processing
// ----------------------------------------------------------------------

/// Center the measured title above the diagram's bounding box.
pub fn position_title(layout: &mut Layout, model: &AdapterModel) {
    let Some((tw, th)) = model.title else { return };
    let boxes: Vec<(f64, f64, f64)> = layout
        .nodes
        .values()
        .map(|n| (n.x, n.y, n.width))
        .chain(layout.groups.values().map(|g| (g.x, g.y, g.width)))
        .collect();
    if boxes.is_empty() {
        return;
    }
    let min_x = boxes.iter().map(|b| b.0).fold(f64::INFINITY, f64::min);
    let max_x = boxes.iter().map(|b| b.0 + b.2).fold(f64::NEG_INFINITY, f64::max);
    let min_y = boxes.iter().map(|b| b.1).fold(f64::INFINITY, f64::min);
    layout.nodes.insert(
        "__title__".into(),
        LayoutNode {
            id: "__title__".into(),
            x: min_x + ((max_x - min_x) - tw) / 2.0,
            y: min_y - th,
            width: tw,
            height: th,
            xlabel_pos: None,
        },
    );
}

/// Fan apart overlapping parallel edge trunks (vertical then horizontal):
/// maximal same-coordinate runs from different edges within `min_gap` of
/// each other and overlapping in range move to `anchor + k·min_gap`,
/// longest trunk anchored, first/last segments pinned.
pub fn separate_overlapping_edges(layout: &mut Layout, min_gap: f64) {
    if min_gap <= 0.0 || layout.edges.len() < 2 {
        return;
    }
    separate_trunks(&mut layout.edges, min_gap, true);
    separate_trunks(&mut layout.edges, min_gap, false);
}

struct Trunk {
    edge_idx: usize,
    start: usize,
    end: usize,
    pos: f64,
    range_min: f64,
    range_max: f64,
}

fn separate_trunks(edges: &mut [LayoutEdge], min_gap: f64, vertical: bool) {
    let coord = |p: &(f64, f64)| if vertical { p.0 } else { p.1 };
    let range = |p: &(f64, f64)| if vertical { p.1 } else { p.0 };

    let mut trunks: Vec<Trunk> = Vec::new();
    for (ei, edge) in edges.iter().enumerate() {
        let pts = &edge.points;
        if pts.len() < 2 {
            continue;
        }
        let mut i = 0;
        while i < pts.len() {
            let c = coord(&pts[i]);
            let mut j = i + 1;
            while j < pts.len() && (coord(&pts[j]) - c).abs() < 0.5 {
                j += 1;
            }
            if j - i >= 2 {
                let (mut r_min, mut r_max) = (f64::INFINITY, f64::NEG_INFINITY);
                for p in &pts[i..j] {
                    r_min = r_min.min(range(p));
                    r_max = r_max.max(range(p));
                }
                if r_max - r_min > 0.5 {
                    trunks.push(Trunk {
                        edge_idx: ei,
                        start: i,
                        end: j - 1,
                        pos: c,
                        range_min: r_min,
                        range_max: r_max,
                    });
                }
            }
            i = j;
        }
    }
    if trunks.len() < 2 {
        return;
    }

    // Union-find over conflicting trunks.
    let mut parent: Vec<usize> = (0..trunks.len()).collect();
    fn find(parent: &mut Vec<usize>, mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let mut by_pos: Vec<usize> = (0..trunks.len()).collect();
    by_pos.sort_by(|&a, &b| trunks[a].pos.total_cmp(&trunks[b].pos));
    for i in 0..by_pos.len() {
        for j in i + 1..by_pos.len() {
            if trunks[by_pos[j]].pos - trunks[by_pos[i]].pos >= min_gap {
                break;
            }
            let (ti, tj) = (&trunks[by_pos[i]], &trunks[by_pos[j]]);
            if ti.edge_idx == tj.edge_idx {
                continue;
            }
            if ti.range_max.min(tj.range_max) > ti.range_min.max(tj.range_min) {
                let (a, b) = (find(&mut parent, by_pos[i]), find(&mut parent, by_pos[j]));
                parent[a] = b;
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..trunks.len() {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    // Deterministic group order (by first member).
    let mut group_list: Vec<Vec<usize>> = groups.into_values().collect();
    group_list.sort_by_key(|members| *members.iter().min().unwrap());

    for member_idxs in group_list {
        if member_idxs.len() < 2 {
            continue;
        }
        let mut members: Vec<usize> = member_idxs;
        members.sort_by(|&a, &b| {
            let ea = trunks[a].range_max - trunks[a].range_min;
            let eb = trunks[b].range_max - trunks[b].range_min;
            eb.total_cmp(&ea)
        });
        let anchor = trunks[members[0]].pos;
        for (k, &mi) in members.iter().enumerate().skip(1) {
            let t = &trunks[mi];
            let n_points = edges[t.edge_idx].points.len();
            if t.start == 0 || t.end == n_points - 1 {
                continue;
            }
            let new_pos = anchor + k as f64 * min_gap;
            let delta = new_pos - t.pos;
            if delta.abs() < 0.01 {
                continue;
            }
            for p in &mut edges[t.edge_idx].points[t.start..=t.end] {
                if vertical {
                    p.0 += delta;
                } else {
                    p.1 += delta;
                }
            }
        }
    }
}

// ----------------------------------------------------------------------
// Full pipeline
// ----------------------------------------------------------------------

/// Run the complete adapter pipeline on a measured model: pass 1, node
/// positions, pass 2, extraction, and post-processing. Uses the ported
/// ELK engine (`hierarchical::layout_compound`) for both passes.
pub fn layout_model(model: &AdapterModel) -> Layout {
    let pass1 = to_elk_simple(model);
    let laid1 = run_engine(&pass1);
    let positions = collect_node_positions(&laid1);

    let pass2 = to_elk(model, &positions);
    let laid2 = run_engine(&pass2);

    let mut layout = extract(&laid2, model);
    position_title(&mut layout, model);
    separate_overlapping_edges(&mut layout, model.spacing.edge_gap);
    layout
}

/// One engine run: import → compound layout → export back onto the input
/// tree shape.
pub fn run_engine(input: &json::ElkNode) -> json::ElkNode {
    let mut arena = LGraphArena::default();
    let top = transform::import_graph(&mut arena, input);
    let result = hierarchical::layout_compound(&mut arena, top);
    transform::apply_layout_compound(&arena, top, input, &result.reference_graphs)
}
