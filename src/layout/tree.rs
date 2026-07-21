//! Faithful Rust port of the tree / mind-map layout geometry from
//! `components/src/tree.typ`.
//!
//! The Typst painter used to both lay out and paint tree diagrams; this
//! module extracts the layout half so the coordinates are computed in
//! Rust — mirroring the `record_graph` architecture — and Typst (or the
//! web renderer) only paints. The port is deliberately line-for-line
//! faithful to `tree.typ`'s "recursive bottom-up stacking + symmetric
//! padding" scheme so the rendered output stays visually identical:
//!
//! - `layout_down` ↔ the `direction == "down"` branch of `tree()`
//! - `layout_horiz` ↔ the `"right"` / `"left"` branch (`mirror` flips)
//! - `layout_mindmap` ↔ `mindmap()`'s two-column composition
//!
//! Every subtree becomes a [`Blob`]: a self-contained canvas with its
//! root at the cross-axis center of its bounding box (the symmetric
//! `half-w` padding invariant), so a parent connector aimed at the
//! blob's near-edge center always lands on the subtree's actual root.
//!
//! Elbow connectors are emitted as one 4-point polyline per child —
//! geometrically identical to the three overlapping `line()` calls the
//! Typst painter used to draw.
//!
//! Nothing in here touches Typst: node sizes come in via
//! [`TreeLayoutInput::size`] (from the measure double-pass on the CLI
//! path, from DOM measurement on the web path), and gaps are derived
//! from the resolved `em` the caller supplies.

/// Gap configuration, mirroring the `tree.typ` / `mindmap()` defaults.
/// All fields are in the same unit as the node sizes (Typst pt on the
/// CLI path, CSS px on the web path).
#[derive(Clone, Copy, Debug)]
pub struct TreeConfig {
    /// Sibling gap on the cross axis (`x-gap: 1.6em`).
    pub x_gap: f64,
    /// Root-to-children gap on the main axis (`y-gap: 2.2em`).
    pub y_gap: f64,
    /// Mindmap: vertical gap between branches on the same side (`v-gap: 0.8em`).
    pub v_gap: f64,
    /// Mindmap: horizontal clearance between the central root and either
    /// column (`side-gap: 1.2em`).
    pub side_gap: f64,
    pub edge_style: EdgeStyle,
}

impl TreeConfig {
    /// Defaults from `tree.typ` scaled by the resolved size of `1em`.
    pub fn from_em(em: f64) -> Self {
        Self {
            x_gap: 1.6 * em,
            y_gap: 2.2 * em,
            v_gap: 0.8 * em,
            side_gap: 1.2 * em,
            edge_style: EdgeStyle::Elbow,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeStyle {
    /// Orthogonal three-segment route over a shared bus (the default).
    Elbow,
    /// One straight diagonal.
    Line,
}

/// One node of the layout input tree. `id` is caller-assigned (pre-order
/// index into the flat node list on both the codegen and web paths) and
/// is echoed back on [`PlacedNode`] / [`EdgePolyline`].
#[derive(Clone, Debug)]
pub struct TreeLayoutInput {
    pub id: usize,
    /// Natural (width, height) of the rendered node box.
    pub size: (f64, f64),
    pub children: Vec<TreeLayoutInput>,
}

#[derive(Clone, Copy, Debug)]
pub struct PlacedNode {
    pub id: usize,
    /// Top-left corner, absolute within the layout canvas.
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Clone, Debug)]
pub struct EdgePolyline {
    pub from: usize,
    pub to: usize,
    /// Consecutive polyline points; adjacent duplicates already removed
    /// (a zero-width bus collapses to a straight line).
    pub points: Vec<(f64, f64)>,
}

/// Finished layout: canvas extents plus placed nodes and connector
/// polylines, all in absolute canvas coordinates (y grows downward).
#[derive(Clone, Debug, Default)]
pub struct TreeLayout {
    pub width: f64,
    pub height: f64,
    pub nodes: Vec<PlacedNode>,
    pub edges: Vec<EdgePolyline>,
}

/// A laid-out subtree in blob-local coordinates. `root_*` locate the
/// blob's own root node box so parents can aim connectors at it.
struct Blob {
    w: f64,
    h: f64,
    root_id: usize,
    nodes: Vec<PlacedNode>,
    edges: Vec<EdgePolyline>,
}

impl Blob {
    fn leaf(node: &TreeLayoutInput) -> Self {
        Blob {
            w: node.size.0,
            h: node.size.1,
            root_id: node.id,
            nodes: vec![PlacedNode {
                id: node.id,
                x: 0.0,
                y: 0.0,
                w: node.size.0,
                h: node.size.1,
            }],
            edges: Vec::new(),
        }
    }

    /// Merge `child` into `self` translated by `(dx, dy)`.
    fn absorb(&mut self, child: Blob, dx: f64, dy: f64) {
        for mut n in child.nodes {
            n.x += dx;
            n.y += dy;
            self.nodes.push(n);
        }
        for mut e in child.edges {
            for p in &mut e.points {
                p.0 += dx;
                p.1 += dy;
            }
            self.edges.push(e);
        }
    }
}

/// Push `points` as a connector polyline, dropping adjacent duplicates
/// (a zero-width bus segment) and merging collinear runs (a collapsed
/// bus leaves three points on one vertical line) so painters see the
/// minimal equivalent polyline.
fn push_edge(edges: &mut Vec<EdgePolyline>, from: usize, to: usize, points: &[(f64, f64)]) {
    let mut cleaned: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &p in points {
        if cleaned.last().map(|&q| q == p).unwrap_or(false) {
            continue;
        }
        // Replace the middle point of a collinear triple. All connector
        // segments are axis-aligned or a single diagonal, so the exact
        // cross-product test is stable here.
        if cleaned.len() >= 2 {
            let a = cleaned[cleaned.len() - 2];
            let b = cleaned[cleaned.len() - 1];
            let cross = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
            if cross == 0.0 {
                cleaned.pop();
            }
        }
        cleaned.push(p);
    }
    edges.push(EdgePolyline {
        from,
        to,
        points: cleaned,
    });
}

/// Port of the `direction == "down"` branch of `tree()`.
fn layout_down(node: &TreeLayoutInput, cfg: &TreeConfig) -> Blob {
    if node.children.is_empty() {
        return Blob::leaf(node);
    }
    let (root_w, root_h) = node.size;
    let kid_blobs: Vec<Blob> = node.children.iter().map(|c| layout_down(c, cfg)).collect();
    let n = kid_blobs.len();

    let total_kid_w: f64 =
        kid_blobs.iter().map(|b| b.w).sum::<f64>() + cfg.x_gap * (n as f64 - 1.0);

    // Provisional x-cursor per child, then the child's center x.
    let mut provisional_xs = Vec::with_capacity(n);
    let mut px = 0.0;
    for b in &kid_blobs {
        provisional_xs.push(px);
        px += b.w + cfg.x_gap;
    }
    let kid_cx_at = |i: usize| provisional_xs[i] + kid_blobs[i].w / 2.0;

    // Align the root with the "trunk" child: middle child when odd,
    // midpoint of the two middle children when even.
    let desired_root_cx = if n % 2 == 1 {
        kid_cx_at((n - 1) / 2)
    } else {
        let right = n / 2;
        (kid_cx_at(right - 1) + kid_cx_at(right)) / 2.0
    };
    let desired_root_x = desired_root_cx - root_w / 2.0;

    // Symmetric padding: root sits at the exact horizontal center of the
    // blob's bounding box.
    let bbox_left = 0.0f64.min(desired_root_x);
    let bbox_right = total_kid_w.max(desired_root_x + root_w);
    let half_w = (desired_root_cx - bbox_left).max(bbox_right - desired_root_cx);
    let shift = half_w - desired_root_cx;
    let canvas_w = 2.0 * half_w;
    let root_x = desired_root_x + shift;
    let kids_start_x = shift;

    let kid_y = root_h + cfg.y_gap;
    let max_kid_h = kid_blobs.iter().map(|b| b.h).fold(0.0, f64::max);
    let canvas_h = kid_y + max_kid_h;

    let root_cx = root_x + root_w / 2.0;
    let root_by = root_h;
    let mid_y = root_by + cfg.y_gap / 2.0;

    let mut kid_xs = Vec::with_capacity(n);
    let mut acc = kids_start_x;
    for b in &kid_blobs {
        kid_xs.push(acc);
        acc += b.w + cfg.x_gap;
    }

    let mut blob = Blob {
        w: canvas_w,
        h: canvas_h,
        root_id: node.id,
        nodes: vec![PlacedNode {
            id: node.id,
            x: root_x,
            y: 0.0,
            w: root_w,
            h: root_h,
        }],
        edges: Vec::new(),
    };

    for (i, kb) in kid_blobs.into_iter().enumerate() {
        let child_cx = kid_xs[i] + kb.w / 2.0;
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut blob.edges,
                node.id,
                kb.root_id,
                &[
                    (root_cx, root_by),
                    (root_cx, mid_y),
                    (child_cx, mid_y),
                    (child_cx, kid_y),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut blob.edges,
                node.id,
                kb.root_id,
                &[(root_cx, root_by), (child_cx, kid_y)],
            ),
        }
        blob.absorb(kb, kid_xs[i], kid_y);
    }

    blob
}

/// Port of the horizontal (`"right"` / `"left"`) branch of `tree()`.
/// `mirror == true` is `"left"`: the root hugs the right edge and
/// children grow leftward.
fn layout_horiz(node: &TreeLayoutInput, cfg: &TreeConfig, mirror: bool) -> Blob {
    if node.children.is_empty() {
        return Blob::leaf(node);
    }
    let (root_w, root_h) = node.size;
    let kid_blobs: Vec<Blob> = node
        .children
        .iter()
        .map(|c| layout_horiz(c, cfg, mirror))
        .collect();
    let n = kid_blobs.len();

    let total_kid_h: f64 =
        kid_blobs.iter().map(|b| b.h).sum::<f64>() + cfg.y_gap * (n as f64 - 1.0);

    let mut provisional_ys = Vec::with_capacity(n);
    let mut py = 0.0;
    for b in &kid_blobs {
        provisional_ys.push(py);
        py += b.h + cfg.y_gap;
    }
    let kid_cy_at = |i: usize| provisional_ys[i] + kid_blobs[i].h / 2.0;

    let desired_root_cy = if n % 2 == 1 {
        kid_cy_at((n - 1) / 2)
    } else {
        let right = n / 2;
        (kid_cy_at(right - 1) + kid_cy_at(right)) / 2.0
    };
    let desired_root_y = desired_root_cy - root_h / 2.0;

    let bbox_top = 0.0f64.min(desired_root_y);
    let bbox_bottom = total_kid_h.max(desired_root_y + root_h);
    let half_h = (desired_root_cy - bbox_top).max(bbox_bottom - desired_root_cy);
    let shift = half_h - desired_root_cy;
    let canvas_h = 2.0 * half_h;
    let root_y = desired_root_y + shift;
    let kids_start_y = shift;

    let max_kid_w = kid_blobs.iter().map(|b| b.w).fold(0.0, f64::max);
    let canvas_w = root_w + cfg.x_gap + max_kid_w;

    let root_x = if mirror { canvas_w - root_w } else { 0.0 };
    let kid_x = if mirror { 0.0 } else { root_w + cfg.x_gap };

    let root_cy = root_y + root_h / 2.0;
    let root_out_x = if mirror { root_x } else { root_x + root_w };
    let mid_x = if mirror {
        root_out_x - cfg.x_gap / 2.0
    } else {
        root_out_x + cfg.x_gap / 2.0
    };

    let mut kid_ys = Vec::with_capacity(n);
    let mut acc = kids_start_y;
    for b in &kid_blobs {
        kid_ys.push(acc);
        acc += b.h + cfg.y_gap;
    }

    let mut blob = Blob {
        w: canvas_w,
        h: canvas_h,
        root_id: node.id,
        nodes: vec![PlacedNode {
            id: node.id,
            x: root_x,
            y: root_y,
            w: root_w,
            h: root_h,
        }],
        edges: Vec::new(),
    };

    for (i, kb) in kid_blobs.into_iter().enumerate() {
        let child_cy = kid_ys[i] + kb.h / 2.0;
        // Each child blob keeps its own root on the side facing the
        // parent (mirror: right edge; else: left edge).
        let child_in_x = if mirror { kid_x + kb.w } else { kid_x };
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut blob.edges,
                node.id,
                kb.root_id,
                &[
                    (root_out_x, root_cy),
                    (mid_x, root_cy),
                    (mid_x, child_cy),
                    (child_in_x, child_cy),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut blob.edges,
                node.id,
                kb.root_id,
                &[(root_out_x, root_cy), (child_in_x, child_cy)],
            ),
        }
        blob.absorb(kb, kid_x, kid_ys[i]);
    }

    blob
}

/// Lay out a WBS-style top-down tree. Matches the old
/// `#align(center, tree(node[root], …))` output.
pub fn layout_wbs(root: &TreeLayoutInput, cfg: &TreeConfig) -> TreeLayout {
    let blob = layout_down(root, cfg);
    TreeLayout {
        width: blob.w,
        height: blob.h,
        nodes: blob.nodes,
        edges: blob.edges,
    }
}

/// Lay out a mind map: central `root` plus left- and right-growing
/// first-level branches. Port of `mindmap()` in `tree.typ`.
///
/// `lefts` / `rights` are the first-level subtrees in top-to-bottom
/// stacking order (matching the codegen's partition).
pub fn layout_mindmap(
    root: &TreeLayoutInput,
    lefts: &[TreeLayoutInput],
    rights: &[TreeLayoutInput],
    cfg: &TreeConfig,
) -> TreeLayout {
    let (root_w, root_h) = root.size;

    let left_blobs: Vec<Blob> = lefts.iter().map(|b| layout_horiz(b, cfg, true)).collect();
    let right_blobs: Vec<Blob> = rights.iter().map(|b| layout_horiz(b, cfg, false)).collect();

    let stack_h = |blobs: &[Blob]| -> f64 {
        if blobs.is_empty() {
            0.0
        } else {
            blobs.iter().map(|b| b.h).sum::<f64>() + cfg.v_gap * (blobs.len() as f64 - 1.0)
        }
    };
    let left_stack_h = stack_h(&left_blobs);
    let right_stack_h = stack_h(&right_blobs);
    let left_max_w = left_blobs.iter().map(|b| b.w).fold(0.0, f64::max);
    let right_max_w = right_blobs.iter().map(|b| b.w).fold(0.0, f64::max);

    let canvas_h = left_stack_h.max(right_stack_h).max(root_h);

    let left_col_w = if left_blobs.is_empty() {
        0.0
    } else {
        left_max_w + cfg.side_gap
    };
    let right_col_w = if right_blobs.is_empty() {
        0.0
    } else {
        right_max_w + cfg.side_gap
    };
    let canvas_w = left_col_w + root_w + right_col_w;

    let root_x = left_col_w;
    let root_y = (canvas_h - root_h) / 2.0;
    let root_cy = root_y + root_h / 2.0;

    let root_left_anchor_x = root_x;
    let root_right_anchor_x = root_x + root_w;

    let stack_start_y = |h: f64| (canvas_h - h) / 2.0;

    let mut left_ys = Vec::with_capacity(left_blobs.len());
    let mut acc = stack_start_y(left_stack_h);
    for b in &left_blobs {
        left_ys.push(acc);
        acc += b.h + cfg.v_gap;
    }
    let mut right_ys = Vec::with_capacity(right_blobs.len());
    let mut acc_r = stack_start_y(right_stack_h);
    for b in &right_blobs {
        right_ys.push(acc_r);
        acc_r += b.h + cfg.v_gap;
    }

    // Branch anchors: inner edge of the blob column at each blob's
    // vertical center.
    let left_anchor_x = left_max_w;
    let right_anchor_x = canvas_w - right_max_w;

    let mid_x_left = root_left_anchor_x - cfg.side_gap / 2.0;
    let mid_x_right = root_right_anchor_x + cfg.side_gap / 2.0;

    let mut out = TreeLayout {
        width: canvas_w,
        height: canvas_h,
        nodes: vec![PlacedNode {
            id: root.id,
            x: root_x,
            y: root_y,
            w: root_w,
            h: root_h,
        }],
        edges: Vec::new(),
    };

    for (i, b) in left_blobs.into_iter().enumerate() {
        let anchor_y = left_ys[i] + b.h / 2.0;
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[
                    (root_left_anchor_x, root_cy),
                    (mid_x_left, root_cy),
                    (mid_x_left, anchor_y),
                    (left_anchor_x, anchor_y),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[(root_left_anchor_x, root_cy), (left_anchor_x, anchor_y)],
            ),
        }
        // Right-align left blobs within the left column so every
        // left-branch root shares a trunk x.
        let dx = left_max_w - b.w;
        let dy = left_ys[i];
        merge_blob(&mut out, b, dx, dy);
    }

    for (i, b) in right_blobs.into_iter().enumerate() {
        let anchor_y = right_ys[i] + b.h / 2.0;
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[
                    (root_right_anchor_x, root_cy),
                    (mid_x_right, root_cy),
                    (mid_x_right, anchor_y),
                    (right_anchor_x, anchor_y),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[(root_right_anchor_x, root_cy), (right_anchor_x, anchor_y)],
            ),
        }
        let dx = canvas_w - right_max_w;
        let dy = right_ys[i];
        merge_blob(&mut out, b, dx, dy);
    }

    out
}

fn merge_blob(out: &mut TreeLayout, blob: Blob, dx: f64, dy: f64) {
    for mut n in blob.nodes {
        n.x += dx;
        n.y += dy;
        out.nodes.push(n);
    }
    for mut e in blob.edges {
        for p in &mut e.points {
            p.0 += dx;
            p.1 += dy;
        }
        out.edges.push(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: usize, w: f64, h: f64) -> TreeLayoutInput {
        TreeLayoutInput {
            id,
            size: (w, h),
            children: Vec::new(),
        }
    }

    fn cfg() -> TreeConfig {
        TreeConfig::from_em(10.0)
    }

    fn node_by_id(l: &TreeLayout, id: usize) -> PlacedNode {
        *l.nodes.iter().find(|n| n.id == id).unwrap()
    }

    #[test]
    fn single_node_is_its_own_canvas() {
        let root = leaf(0, 50.0, 20.0);
        let l = layout_wbs(&root, &cfg());
        assert_eq!((l.width, l.height), (50.0, 20.0));
        assert_eq!(l.nodes.len(), 1);
        assert!(l.edges.is_empty());
    }

    #[test]
    fn down_root_centered_over_symmetric_children() {
        // Two equal children → root centered, canvas hugs the pair.
        let root = TreeLayoutInput {
            id: 0,
            size: (40.0, 20.0),
            children: vec![leaf(1, 40.0, 20.0), leaf(2, 40.0, 20.0)],
        };
        let c = cfg();
        let l = layout_wbs(&root, &c);
        let total_kid_w = 40.0 + c.x_gap + 40.0;
        assert!((l.width - total_kid_w).abs() < 1e-9);
        let r = node_by_id(&l, 0);
        // Root center == canvas center.
        assert!((r.x + r.w / 2.0 - l.width / 2.0).abs() < 1e-9);
        // Children on the same row below root + y-gap.
        assert!((node_by_id(&l, 1).y - (20.0 + c.y_gap)).abs() < 1e-9);
        assert!((node_by_id(&l, 2).y - (20.0 + c.y_gap)).abs() < 1e-9);
    }

    #[test]
    fn down_lopsided_pads_symmetrically() {
        // One wide, one narrow child: canvas inflates so the root center
        // still sits at the bbox center (the tree.typ invariant).
        let root = TreeLayoutInput {
            id: 0,
            size: (30.0, 20.0),
            children: vec![leaf(1, 100.0, 20.0), leaf(2, 20.0, 20.0)],
        };
        let l = layout_wbs(&root, &cfg());
        let r = node_by_id(&l, 0);
        assert!(
            (r.x + r.w / 2.0 - l.width / 2.0).abs() < 1e-9,
            "root must sit at canvas center: root_cx={} canvas_w={}",
            r.x + r.w / 2.0,
            l.width
        );
    }

    #[test]
    fn odd_child_count_aligns_root_to_middle_child() {
        let root = TreeLayoutInput {
            id: 0,
            size: (30.0, 20.0),
            children: vec![leaf(1, 40.0, 20.0), leaf(2, 60.0, 20.0), leaf(3, 40.0, 20.0)],
        };
        let l = layout_wbs(&root, &cfg());
        let r = node_by_id(&l, 0);
        let mid = node_by_id(&l, 2);
        assert!(
            (r.x + r.w / 2.0 - (mid.x + mid.w / 2.0)).abs() < 1e-9,
            "root trunk must align with middle child"
        );
    }

    #[test]
    fn elbow_edge_is_four_point_polyline() {
        let root = TreeLayoutInput {
            id: 0,
            size: (30.0, 20.0),
            children: vec![leaf(1, 30.0, 20.0), leaf(2, 30.0, 20.0)],
        };
        let c = cfg();
        let l = layout_wbs(&root, &c);
        assert_eq!(l.edges.len(), 2);
        let e = &l.edges[0];
        assert_eq!(e.points.len(), 4);
        // Starts at root bottom-center, ends at child top-center.
        let r = node_by_id(&l, 0);
        let k = node_by_id(&l, e.to);
        assert!((e.points[0].0 - (r.x + r.w / 2.0)).abs() < 1e-9);
        assert!((e.points[0].1 - r.h).abs() < 1e-9);
        assert!((e.points[3].0 - (k.x + k.w / 2.0)).abs() < 1e-9);
        assert!((e.points[3].1 - k.y).abs() < 1e-9);
        // Bus sits at mid-y.
        let mid_y = r.h + c.y_gap / 2.0;
        assert!((e.points[1].1 - mid_y).abs() < 1e-9);
        assert!((e.points[2].1 - mid_y).abs() < 1e-9);
    }

    #[test]
    fn aligned_trunk_collapses_to_straight_line() {
        // Single child directly under the root → the bus has zero width
        // and the polyline dedupes to a 2-point straight segment.
        let root = TreeLayoutInput {
            id: 0,
            size: (30.0, 20.0),
            children: vec![leaf(1, 30.0, 20.0)],
        };
        let l = layout_wbs(&root, &cfg());
        assert_eq!(l.edges[0].points.len(), 2);
    }

    #[test]
    fn horiz_right_stacks_children_vertically() {
        let branch = TreeLayoutInput {
            id: 0,
            size: (40.0, 20.0),
            children: vec![leaf(1, 50.0, 20.0), leaf(2, 50.0, 20.0)],
        };
        let c = cfg();
        let blob = layout_horiz(&branch, &c, false);
        // Root vertically centered against the child stack.
        let r = *blob.nodes.iter().find(|n| n.id == 0).unwrap();
        assert!((r.y + r.h / 2.0 - blob.h / 2.0).abs() < 1e-9);
        // Children start after root + x-gap.
        let k1 = *blob.nodes.iter().find(|n| n.id == 1).unwrap();
        assert!((k1.x - (40.0 + c.x_gap)).abs() < 1e-9);
    }

    #[test]
    fn horiz_left_mirrors_root_to_right_edge() {
        let branch = TreeLayoutInput {
            id: 0,
            size: (40.0, 20.0),
            children: vec![leaf(1, 50.0, 20.0)],
        };
        let blob = layout_horiz(&branch, &cfg(), true);
        let r = *blob.nodes.iter().find(|n| n.id == 0).unwrap();
        assert!((r.x + r.w - blob.w).abs() < 1e-9, "left root hugs right edge");
        let k = *blob.nodes.iter().find(|n| n.id == 1).unwrap();
        assert!((k.x - 0.0).abs() < 1e-9, "left children hug left edge");
    }

    #[test]
    fn mindmap_columns_center_against_taller_side() {
        let root = leaf(0, 40.0, 20.0);
        let lefts = vec![leaf(1, 30.0, 20.0)];
        let rights = vec![leaf(2, 30.0, 20.0), leaf(3, 30.0, 20.0)];
        let c = cfg();
        let l = layout_mindmap(&root, &lefts, &rights, &c);
        let right_stack = 20.0 + c.v_gap + 20.0;
        assert!((l.height - right_stack).abs() < 1e-9);
        // Root vertically centered.
        let r = node_by_id(&l, 0);
        assert!((r.y + r.h / 2.0 - l.height / 2.0).abs() < 1e-9);
        // Left leaf right-aligned against the left column inner edge.
        let left = node_by_id(&l, 1);
        assert!((left.x + left.w - 30.0).abs() < 1e-9);
        // Canvas: left col + root + right col.
        assert!((l.width - (30.0 + c.side_gap + 40.0 + c.side_gap + 30.0)).abs() < 1e-9);
    }

    #[test]
    fn mindmap_empty_side_contributes_zero_width() {
        let root = leaf(0, 40.0, 20.0);
        let rights = vec![leaf(1, 30.0, 20.0)];
        let l = layout_mindmap(&root, &[], &rights, &cfg());
        let r = node_by_id(&l, 0);
        assert!((r.x - 0.0).abs() < 1e-9, "no left column → root at x=0");
    }

    #[test]
    fn mindmap_edge_lands_on_branch_anchor() {
        let root = leaf(0, 40.0, 20.0);
        let branch = TreeLayoutInput {
            id: 1,
            size: (30.0, 20.0),
            children: vec![leaf(2, 30.0, 20.0)],
        };
        let c = cfg();
        let l = layout_mindmap(&root, &[], &[branch], &c);
        let e = l.edges.iter().find(|e| e.from == 0 && e.to == 1).unwrap();
        let b1 = node_by_id(&l, 1);
        let last = *e.points.last().unwrap();
        // Root→branch connector ends at the branch root's left edge center.
        assert!((last.0 - b1.x).abs() < 1e-9);
        assert!((last.1 - (b1.y + b1.h / 2.0)).abs() < 1e-9);
    }
}
