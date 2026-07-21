//! Tree / mind-map layout — v2.
//!
//! v1 was a line-for-line port of `components/src/tree.typ`'s "naive
//! stacking + symmetric padding" scheme (extraction verified pixel-exact
//! against the Typst-side layout before this rewrite; see
//! `docs/mindmap-web-interactive-design.md` §2.3). v2 replaces the
//! packing with a Reingold–Tilford-style tidy layout and adds the
//! PlantUML-faithful WBS outline mode:
//!
//! - **Tidy packing** (`pack_down` / recursive `layout_horiz`): sibling
//!   subtrees are separated by scanning their actual content rectangles
//!   (nodes + connector bounding boxes) instead of rigid bounding boxes,
//!   so a narrow subtree tucks under a wide sibling's overhang. The
//!   parent sits centered over the span of its children's root anchors
//!   (the RT convention) and every blob's canvas is tight — the v1
//!   symmetric-padding invariant is gone, parents aim connectors at each
//!   child blob's *actual* root anchor instead.
//!
//! - **WBS outline** (`outline_blob`): matches PlantUML `@startwbs`
//!   geometry (verified against PlantUML 1.2024.7 renders): level-2
//!   children spread horizontally under the root; level-3+ children hang
//!   off a vertical trunk dropped from their parent's bottom center —
//!   `<`-marked nodes stack in a left column, `>` / unmarked in a right
//!   column, each column an independent vertical stack, every child
//!   connected to the trunk by a horizontal stub at its center.
//!
//! Mind maps keep their two-column composition (`layout_mindmap`); only
//! the branches' internal packing changed.
//!
//! Nothing in here touches Typst: node sizes come in via
//! [`TreeLayoutInput::size`] (from the measure double-pass on the CLI
//! path, from DOM measurement on the web path), and gaps are derived
//! from the resolved `em` the caller supplies.

/// Gap configuration. All fields are in the same unit as the node sizes
/// (Typst pt on the CLI path, CSS px on the web path).
#[derive(Clone, Copy, Debug)]
pub struct TreeConfig {
    /// Sibling gap on the cross axis (`1.6em`).
    pub x_gap: f64,
    /// Parent-to-children gap on the main axis (`2.2em`).
    pub y_gap: f64,
    /// Row gap in vertical stacks: mindmap branch columns and WBS
    /// outline rows (`0.8em`).
    pub v_gap: f64,
    /// Horizontal clearance: mindmap root-to-column gap and WBS outline
    /// trunk-to-box stub length (`1.2em`).
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
    /// Orthogonal route (elbow bus / outline trunk+stub — the default).
    Elbow,
    /// One straight diagonal (down / horizontal packing only).
    Line,
}

/// Which side of the trunk a WBS outline child hangs on. PlantUML's
/// `<` marker maps to `Left`; `>` and unmarked map to `Right`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Side {
    Left,
    #[default]
    Right,
}

/// One node of the layout input tree. `id` is caller-assigned (pre-order
/// index into the flat node list on both the codegen and web paths) and
/// is echoed back on [`PlacedNode`] / [`EdgePolyline`].
#[derive(Clone, Debug)]
pub struct TreeLayoutInput {
    pub id: usize,
    /// Natural (width, height) of the rendered node box.
    pub size: (f64, f64),
    /// WBS outline column. Ignored by mindmap layout and by the WBS
    /// root's direct children (level 2 is always horizontal, matching
    /// PlantUML).
    pub side: Side,
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
    /// A WBS trunk segment carries `from == to == parent id`; stubs and
    /// elbow buses carry the child's id in `to`.
    pub from: usize,
    pub to: usize,
    /// Consecutive polyline points; adjacent duplicates and collinear
    /// runs already removed.
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

// ---------------------------------------------------------------------------
// Blob: a laid-out subtree in local coordinates
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
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

    fn root(&self) -> &PlacedNode {
        self.nodes
            .iter()
            .find(|n| n.id == self.root_id)
            .expect("blob root present")
    }

    fn translate(&mut self, dx: f64, dy: f64) {
        for n in &mut self.nodes {
            n.x += dx;
            n.y += dy;
        }
        for e in &mut self.edges {
            for p in &mut e.points {
                p.0 += dx;
                p.1 += dy;
            }
        }
    }

    fn absorb(&mut self, mut child: Blob, dx: f64, dy: f64) {
        child.translate(dx, dy);
        self.nodes.append(&mut child.nodes);
        self.edges.append(&mut child.edges);
    }

    /// Content rectangles for separation scans: node boxes plus
    /// connector bounding boxes (inflated by half the stroke so
    /// zero-thickness segments still register overlap).
    fn rects(&self) -> Vec<Rect> {
        rects_of(&self.nodes, &self.edges)
    }

    /// Shift content so the minimum x over all content is 0, and set
    /// `w` / `h` to the tight extents.
    fn normalize(&mut self) {
        let rects = self.rects();
        let min_x = rects.iter().map(|r| r.x0).fold(f64::INFINITY, f64::min);
        let min_y = rects.iter().map(|r| r.y0).fold(f64::INFINITY, f64::min);
        let (min_x, min_y) = (
            if min_x.is_finite() { min_x } else { 0.0 },
            if min_y.is_finite() { min_y } else { 0.0 },
        );
        if min_x != 0.0 || min_y != 0.0 {
            self.translate(-min_x, -min_y);
        }
        let rects = self.rects();
        self.w = rects.iter().map(|r| r.x1).fold(0.0, f64::max);
        self.h = rects.iter().map(|r| r.y1).fold(0.0, f64::max);
    }

    fn into_layout(self) -> TreeLayout {
        TreeLayout {
            width: self.w,
            height: self.h,
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

/// Half the connector stroke: bounding boxes of pure-horizontal /
/// pure-vertical segments get this much thickness so they participate
/// in overlap scans.
const EDGE_INFLATE: f64 = 0.4;

fn rects_of(nodes: &[PlacedNode], edges: &[EdgePolyline]) -> Vec<Rect> {
    let mut out: Vec<Rect> = nodes
        .iter()
        .map(|n| Rect {
            x0: n.x,
            y0: n.y,
            x1: n.x + n.w,
            y1: n.y + n.h,
        })
        .collect();
    for e in edges {
        let xs: Vec<f64> = e.points.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = e.points.iter().map(|p| p.1).collect();
        out.push(Rect {
            x0: xs.iter().copied().fold(f64::INFINITY, f64::min) - EDGE_INFLATE,
            y0: ys.iter().copied().fold(f64::INFINITY, f64::min) - EDGE_INFLATE,
            x1: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max) + EDGE_INFLATE,
            y1: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max) + EDGE_INFLATE,
        });
    }
    out
}

/// Minimal x-shift for `incoming` so every pair that overlaps in y
/// keeps `gap` clearance from `placed`. Returns `f64::NEG_INFINITY`
/// when nothing overlaps (caller decides the fallback).
fn min_shift_x(placed: &[Rect], incoming: &[Rect], gap: f64) -> f64 {
    let mut dx = f64::NEG_INFINITY;
    for a in placed {
        for b in incoming {
            if a.y0 < b.y1 && b.y0 < a.y1 {
                dx = dx.max(a.x1 + gap - b.x0);
            }
        }
    }
    dx
}

/// Transpose of [`min_shift_x`]: minimal y-shift given x-overlap.
fn min_shift_y(placed: &[Rect], incoming: &[Rect], gap: f64) -> f64 {
    let mut dy = f64::NEG_INFINITY;
    for a in placed {
        for b in incoming {
            if a.x0 < b.x1 && b.x0 < a.x1 {
                dy = dy.max(a.y1 + gap - b.y0);
            }
        }
    }
    dy
}

/// Push a connector polyline, dropping adjacent duplicates and merging
/// collinear runs so painters see the minimal equivalent polyline.
fn push_edge(edges: &mut Vec<EdgePolyline>, from: usize, to: usize, points: &[(f64, f64)]) {
    let mut cleaned: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &p in points {
        if cleaned.last().map(|&q| q == p).unwrap_or(false) {
            continue;
        }
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
    if cleaned.len() < 2 {
        return;
    }
    edges.push(EdgePolyline {
        from,
        to,
        points: cleaned,
    });
}

// ---------------------------------------------------------------------------
// Tidy packing — down direction
// ---------------------------------------------------------------------------

/// Pack pre-built child blobs left-to-right under a root node using
/// content-rect separation, center the root over the span of child root
/// anchors, and emit the connectors. Children are top-aligned at
/// `root_h + y_gap` (nothing inside a child blob rises above its own
/// root's top, so the connector band between the rows stays clear).
fn pack_down(
    root_id: usize,
    root_size: (f64, f64),
    kid_blobs: Vec<Blob>,
    cfg: &TreeConfig,
) -> Blob {
    let (root_w, root_h) = root_size;
    if kid_blobs.is_empty() {
        return Blob {
            w: root_w,
            h: root_h,
            root_id,
            nodes: vec![PlacedNode {
                id: root_id,
                x: 0.0,
                y: 0.0,
                w: root_w,
                h: root_h,
            }],
            edges: Vec::new(),
        };
    }

    let kid_y = root_h + cfg.y_gap;

    // Place each child blob as far left as its content allows.
    let mut placed_rects: Vec<Rect> = Vec::new();
    let mut kid_dxs: Vec<f64> = Vec::new();
    for kb in &kid_blobs {
        let incoming: Vec<Rect> = kb
            .rects()
            .into_iter()
            .map(|r| Rect {
                y0: r.y0 + kid_y,
                y1: r.y1 + kid_y,
                ..r
            })
            .collect();
        let dx = if placed_rects.is_empty() {
            0.0
        } else {
            let s = min_shift_x(&placed_rects, &incoming, cfg.x_gap);
            if s.is_finite() {
                s
            } else {
                // No y-overlap with anything placed — e.g. a folded
                // phantom collapses to a zero-size blob whose rects
                // never register overlap. Preserve source order by
                // slotting it after everything placed so far.
                let placed_max = placed_rects.iter().map(|r| r.x1).fold(0.0, f64::max);
                let incoming_min = incoming
                    .iter()
                    .map(|r| r.x0)
                    .fold(f64::INFINITY, f64::min);
                placed_max + cfg.x_gap - if incoming_min.is_finite() { incoming_min } else { 0.0 }
            }
        };
        for r in &incoming {
            placed_rects.push(Rect {
                x0: r.x0 + dx,
                x1: r.x1 + dx,
                ..*r
            });
        }
        kid_dxs.push(dx);
    }

    // Root centered over the span of child root anchors.
    let anchors: Vec<f64> = kid_blobs
        .iter()
        .enumerate()
        .map(|(i, kb)| {
            let r = kb.root();
            kid_dxs[i] + r.x + r.w / 2.0
        })
        .collect();
    let root_cx = (anchors[0] + anchors[anchors.len() - 1]) / 2.0;
    let root_x = root_cx - root_w / 2.0;

    let mut blob = Blob {
        w: 0.0,
        h: 0.0,
        root_id,
        nodes: vec![PlacedNode {
            id: root_id,
            x: root_x,
            y: 0.0,
            w: root_w,
            h: root_h,
        }],
        edges: Vec::new(),
    };

    let root_by = root_h;
    let mid_y = root_by + cfg.y_gap / 2.0;
    for (i, kb) in kid_blobs.into_iter().enumerate() {
        let child_cx = anchors[i];
        let child_id = kb.root_id;
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut blob.edges,
                root_id,
                child_id,
                &[
                    (root_cx, root_by),
                    (root_cx, mid_y),
                    (child_cx, mid_y),
                    (child_cx, kid_y),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut blob.edges,
                root_id,
                child_id,
                &[(root_cx, root_by), (child_cx, kid_y)],
            ),
        }
        blob.absorb(kb, kid_dxs[i], kid_y);
    }

    blob.normalize();
    blob
}

/// Plain recursive top-down tree (every level packed horizontally).
/// Kept for down-direction trees that don't use WBS outline semantics.
fn layout_down_recursive(node: &TreeLayoutInput, cfg: &TreeConfig) -> Blob {
    let kid_blobs = node
        .children
        .iter()
        .map(|c| layout_down_recursive(c, cfg))
        .collect();
    pack_down(node.id, node.size, kid_blobs, cfg)
}

// ---------------------------------------------------------------------------
// Tidy packing — horizontal (mind-map branches)
// ---------------------------------------------------------------------------

/// Rightward-growing branch: children stacked vertically (cross axis),
/// root centered over the span of child anchors, tidy rect separation.
/// `layout_horiz` mirrors the finished blob for left-growing branches.
fn layout_horiz_right(node: &TreeLayoutInput, cfg: &TreeConfig) -> Blob {
    if node.children.is_empty() {
        return Blob::leaf(node);
    }
    let (root_w, root_h) = node.size;
    let kid_blobs: Vec<Blob> = node
        .children
        .iter()
        .map(|c| layout_horiz_right(c, cfg))
        .collect();

    let kid_x = root_w + cfg.x_gap;

    let mut placed_rects: Vec<Rect> = Vec::new();
    let mut kid_dys: Vec<f64> = Vec::new();
    for kb in &kid_blobs {
        let incoming: Vec<Rect> = kb
            .rects()
            .into_iter()
            .map(|r| Rect {
                x0: r.x0 + kid_x,
                x1: r.x1 + kid_x,
                ..r
            })
            .collect();
        let dy = if placed_rects.is_empty() {
            0.0
        } else {
            let s = min_shift_y(&placed_rects, &incoming, cfg.y_gap);
            if s.is_finite() {
                s
            } else {
                // No x-overlap (zero-size blob) — keep source order.
                let placed_max = placed_rects.iter().map(|r| r.y1).fold(0.0, f64::max);
                let incoming_min = incoming
                    .iter()
                    .map(|r| r.y0)
                    .fold(f64::INFINITY, f64::min);
                placed_max + cfg.y_gap - if incoming_min.is_finite() { incoming_min } else { 0.0 }
            }
        };
        for r in &incoming {
            placed_rects.push(Rect {
                y0: r.y0 + dy,
                y1: r.y1 + dy,
                ..*r
            });
        }
        kid_dys.push(dy);
    }

    let anchors: Vec<f64> = kid_blobs
        .iter()
        .enumerate()
        .map(|(i, kb)| {
            let r = kb.root();
            kid_dys[i] + r.y + r.h / 2.0
        })
        .collect();
    let root_cy = (anchors[0] + anchors[anchors.len() - 1]) / 2.0;
    let root_y = root_cy - root_h / 2.0;

    let mut blob = Blob {
        w: 0.0,
        h: 0.0,
        root_id: node.id,
        nodes: vec![PlacedNode {
            id: node.id,
            x: 0.0,
            y: root_y,
            w: root_w,
            h: root_h,
        }],
        edges: Vec::new(),
    };

    let root_out_x = root_w;
    let mid_x = root_out_x + cfg.x_gap / 2.0;
    for (i, kb) in kid_blobs.into_iter().enumerate() {
        let child_cy = anchors[i];
        let child_id = kb.root_id;
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut blob.edges,
                node.id,
                child_id,
                &[
                    (root_out_x, root_cy),
                    (mid_x, root_cy),
                    (mid_x, child_cy),
                    (kid_x, child_cy),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut blob.edges,
                node.id,
                child_id,
                &[(root_out_x, root_cy), (kid_x, child_cy)],
            ),
        }
        blob.absorb(kb, kid_x, kid_dys[i]);
    }

    blob.normalize();
    blob
}

/// Mirror a blob around its vertical center line (x → w - x).
fn flip_x(blob: &mut Blob) {
    let w = blob.w;
    for n in &mut blob.nodes {
        n.x = w - (n.x + n.w);
    }
    for e in &mut blob.edges {
        for p in &mut e.points {
            p.0 = w - p.0;
        }
    }
}

fn layout_horiz(node: &TreeLayoutInput, cfg: &TreeConfig, mirror: bool) -> Blob {
    let mut blob = layout_horiz_right(node, cfg);
    if mirror {
        flip_x(&mut blob);
    }
    blob
}

// ---------------------------------------------------------------------------
// WBS outline (PlantUML level-3+ geometry)
// ---------------------------------------------------------------------------

/// One node plus its children hanging off a vertical trunk: `<`-side
/// children in a left column, others in a right column, both columns
/// independent vertical stacks connected to the trunk by horizontal
/// stubs. Built in trunk-local coordinates (trunk at x = 0), then
/// normalized.
fn outline_blob(node: &TreeLayoutInput, cfg: &TreeConfig) -> Blob {
    let (nw, nh) = node.size;
    if node.children.is_empty() {
        return Blob::leaf(node);
    }

    let stub = cfg.side_gap;
    let row_gap = cfg.v_gap;

    let mut blob = Blob {
        w: 0.0,
        h: 0.0,
        root_id: node.id,
        nodes: vec![PlacedNode {
            id: node.id,
            x: -nw / 2.0,
            y: 0.0,
            w: nw,
            h: nh,
        }],
        edges: Vec::new(),
    };

    // Build both columns in trunk-local coords. Each entry: (blob, dx,
    // dy, attach y). Left column first — it is placed as computed; the
    // right column then shifts right if any left subtree pokes across
    // the trunk (each column is confined to its half-plane with
    // x_gap/2 clearance, which also keeps the trunk line itself clear).
    let place_column = |sides: Side| -> Vec<(Blob, f64, f64, f64)> {
        let mut cursor = nh + row_gap;
        let mut out = Vec::new();
        for child in node.children.iter().filter(|c| c.side == sides) {
            let cb = outline_blob(child, cfg);
            let r = cb.root();
            let dx = match sides {
                // Box's inner edge lands `stub` away from the trunk.
                Side::Left => -stub - (r.x + r.w),
                Side::Right => stub - r.x,
            };
            let dy = cursor - r.y; // child root box top at cursor
            let attach_y = dy + r.y + r.h / 2.0;
            cursor = dy + cb.h + row_gap;
            out.push((cb, dx, dy, attach_y));
        }
        out
    };

    let left_col = place_column(Side::Left);
    let right_col = place_column(Side::Right);

    let col_rects = |col: &[(Blob, f64, f64, f64)]| -> Vec<Rect> {
        col.iter()
            .flat_map(|(b, dx, dy, _)| {
                b.rects().into_iter().map(move |r| Rect {
                    x0: r.x0 + dx,
                    x1: r.x1 + dx,
                    y0: r.y0 + dy,
                    y1: r.y1 + dy,
                })
            })
            .collect()
    };

    // Confine each column to its half-plane.
    let clear = cfg.x_gap / 2.0;
    let left_rects = col_rects(&left_col);
    let extra_left = left_rects
        .iter()
        .map(|r| r.x1 - (-clear))
        .fold(0.0, f64::max);
    let right_rects = col_rects(&right_col);
    let extra_right = right_rects
        .iter()
        .map(|r| clear - r.x0)
        .fold(0.0, f64::max);

    let mut last_attach: f64 = f64::NEG_INFINITY;
    for (cb, dx, dy, attach_y) in left_col {
        let dx = dx - extra_left;
        let reach = {
            let r = cb.root();
            dx + r.x + r.w
        };
        push_edge(
            &mut blob.edges,
            node.id,
            cb.root_id,
            &[(0.0, attach_y), (reach, attach_y)],
        );
        last_attach = last_attach.max(attach_y);
        blob.absorb(cb, dx, dy);
    }
    for (cb, dx, dy, attach_y) in right_col {
        let dx = dx + extra_right;
        let reach = {
            let r = cb.root();
            dx + r.x
        };
        push_edge(
            &mut blob.edges,
            node.id,
            cb.root_id,
            &[(0.0, attach_y), (reach, attach_y)],
        );
        last_attach = last_attach.max(attach_y);
        blob.absorb(cb, dx, dy);
    }

    // Trunk, drawn once: parent bottom-center down to the last stub.
    // `from == to == node.id` marks it as the parent's own segment.
    push_edge(
        &mut blob.edges,
        node.id,
        node.id,
        &[(0.0, nh), (0.0, last_attach)],
    );

    blob.normalize();
    blob
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Lay out a WBS: root on top, level-2 children packed horizontally
/// (sides ignored there, matching PlantUML), every deeper level in
/// outline form.
pub fn layout_wbs(root: &TreeLayoutInput, cfg: &TreeConfig) -> TreeLayout {
    let kid_blobs: Vec<Blob> = root
        .children
        .iter()
        .map(|c| outline_blob(c, cfg))
        .collect();
    pack_down(root.id, root.size, kid_blobs, cfg).into_layout()
}

/// Lay out a plain top-down tree (all levels horizontal). Used by the
/// generic tree path and by tests; WBS uses [`layout_wbs`].
pub fn layout_down(root: &TreeLayoutInput, cfg: &TreeConfig) -> TreeLayout {
    layout_down_recursive(root, cfg).into_layout()
}

/// Lay out a mind map: central `root` plus left- and right-growing
/// first-level branches, each column a vertical stack of tidy branch
/// blobs.
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

    let mid_x_left = root_left_anchor_x - cfg.side_gap / 2.0;
    let mid_x_right = root_right_anchor_x + cfg.side_gap / 2.0;

    let mut out = Blob {
        w: 0.0,
        h: 0.0,
        root_id: root.id,
        nodes: vec![PlacedNode {
            id: root.id,
            x: root_x,
            y: root_y,
            w: root_w,
            h: root_h,
        }],
        edges: Vec::new(),
    };

    // Left blobs right-aligned within the left column (a left-growing
    // blob's root box hugs its right edge), right blobs left-aligned —
    // so every branch root's inner edge sits on its column's trunk
    // line. Connector anchors use each blob's ACTUAL root center (tidy
    // blobs no longer center the root vertically).
    for (i, b) in left_blobs.into_iter().enumerate() {
        let dx = left_max_w - b.w;
        let dy = left_ys[i];
        let r = b.root();
        let anchor_y = dy + r.y + r.h / 2.0;
        let anchor_x = dx + r.x + r.w;
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[
                    (root_left_anchor_x, root_cy),
                    (mid_x_left, root_cy),
                    (mid_x_left, anchor_y),
                    (anchor_x, anchor_y),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[(root_left_anchor_x, root_cy), (anchor_x, anchor_y)],
            ),
        }
        out.absorb(b, dx, dy);
    }

    for (i, b) in right_blobs.into_iter().enumerate() {
        let dx = canvas_w - right_max_w;
        let dy = right_ys[i];
        let r = b.root();
        let anchor_y = dy + r.y + r.h / 2.0;
        let anchor_x = dx + r.x;
        match cfg.edge_style {
            EdgeStyle::Elbow => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[
                    (root_right_anchor_x, root_cy),
                    (mid_x_right, root_cy),
                    (mid_x_right, anchor_y),
                    (anchor_x, anchor_y),
                ],
            ),
            EdgeStyle::Line => push_edge(
                &mut out.edges,
                root.id,
                b.root_id,
                &[(root_right_anchor_x, root_cy), (anchor_x, anchor_y)],
            ),
        }
        out.absorb(b, dx, dy);
    }

    out.normalize();
    out.into_layout()
}

/// Transpose a finished layout (x ↔ y, w ↔ h, points swapped). Drives
/// `top to bottom direction` mind maps: build the inputs with swapped
/// node sizes, run the ordinary left-right layout, transpose the result
/// — left-side branches come out growing up, right-side down.
pub fn transpose_layout(l: TreeLayout) -> TreeLayout {
    TreeLayout {
        width: l.height,
        height: l.width,
        nodes: l
            .nodes
            .into_iter()
            .map(|n| PlacedNode {
                id: n.id,
                x: n.y,
                y: n.x,
                w: n.h,
                h: n.w,
            })
            .collect(),
        edges: l
            .edges
            .into_iter()
            .map(|mut e| {
                for p in &mut e.points {
                    *p = (p.1, p.0);
                }
                e
            })
            .collect(),
    }
}

/// Stack independent layouts (multi-root mind maps) into one canvas:
/// vertically (each below the previous) or horizontally, `gap` apart,
/// aligned at 0 on the other axis.
pub fn stack_layouts(layouts: Vec<TreeLayout>, gap: f64, horizontal: bool) -> TreeLayout {
    let mut out = TreeLayout::default();
    let mut cursor = 0.0;
    for (i, mut l) in layouts.into_iter().enumerate() {
        let (dx, dy) = if horizontal { (cursor, 0.0) } else { (0.0, cursor) };
        for n in &mut l.nodes {
            n.x += dx;
            n.y += dy;
        }
        for e in &mut l.edges {
            for p in &mut e.points {
                p.0 += dx;
                p.1 += dy;
            }
        }
        out.nodes.append(&mut l.nodes);
        out.edges.append(&mut l.edges);
        if horizontal {
            out.width = cursor + l.width;
            out.height = out.height.max(l.height);
            cursor += l.width + gap;
        } else {
            out.height = cursor + l.height;
            out.width = out.width.max(l.width);
            cursor += l.height + gap;
        }
        let _ = i;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: usize, w: f64, h: f64) -> TreeLayoutInput {
        TreeLayoutInput {
            id,
            size: (w, h),
            side: Side::Right,
            children: Vec::new(),
        }
    }

    fn parent(id: usize, w: f64, h: f64, children: Vec<TreeLayoutInput>) -> TreeLayoutInput {
        TreeLayoutInput {
            id,
            size: (w, h),
            side: Side::Right,
            children,
        }
    }

    fn cfg() -> TreeConfig {
        TreeConfig::from_em(10.0)
    }

    fn node_by_id(l: &TreeLayout, id: usize) -> PlacedNode {
        *l.nodes.iter().find(|n| n.id == id).unwrap()
    }

    fn no_node_overlap(l: &TreeLayout) {
        for (i, a) in l.nodes.iter().enumerate() {
            for b in l.nodes.iter().skip(i + 1) {
                let overlap = a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h
                    && b.y < a.y + a.h;
                assert!(
                    !overlap,
                    "nodes {} and {} overlap: {a:?} vs {b:?}",
                    a.id, b.id
                );
            }
        }
    }

    #[test]
    fn single_node_is_its_own_canvas() {
        let root = leaf(0, 50.0, 20.0);
        let l = layout_down(&root, &cfg());
        assert_eq!((l.width, l.height), (50.0, 20.0));
        assert_eq!(l.nodes.len(), 1);
        assert!(l.edges.is_empty());
    }

    #[test]
    fn down_root_centered_over_child_anchor_span() {
        let root = parent(0, 40.0, 20.0, vec![leaf(1, 40.0, 20.0), leaf(2, 40.0, 20.0)]);
        let c = cfg();
        let l = layout_down(&root, &c);
        let r = node_by_id(&l, 0);
        let k1 = node_by_id(&l, 1);
        let k2 = node_by_id(&l, 2);
        let span_mid = (k1.x + k1.w / 2.0 + k2.x + k2.w / 2.0) / 2.0;
        assert!((r.x + r.w / 2.0 - span_mid).abs() < 1e-9);
        assert!((k1.y - (20.0 + c.y_gap)).abs() < 1e-9);
    }

    #[test]
    fn down_canvas_is_tight_no_symmetric_padding() {
        // v1 inflated the canvas so the root sat at the bbox center;
        // v2 must hug the content instead.
        let root = parent(0, 30.0, 20.0, vec![leaf(1, 100.0, 20.0), leaf(2, 20.0, 20.0)]);
        let c = cfg();
        let l = layout_down(&root, &c);
        let tight = 100.0 + c.x_gap + 20.0;
        assert!(
            (l.width - tight).abs() < 1e-9,
            "canvas should hug children: got {} want {tight}",
            l.width
        );
    }

    #[test]
    fn narrow_subtree_tucks_under_wide_sibling() {
        // Subtree A: tiny root over a wide row of leaves. Subtree B: a
        // single small leaf. With rigid bbox stacking B starts after
        // A's full width; with tidy packing B's root (at the shallow
        // row) only needs to clear A's ROOT box, so total width
        // shrinks.
        let wide = parent(
            1,
            20.0,
            20.0,
            vec![leaf(2, 80.0, 20.0), leaf(3, 80.0, 20.0)],
        );
        let small = leaf(4, 20.0, 20.0);
        let root = parent(0, 20.0, 20.0, vec![wide, small]);
        let c = cfg();
        let l = layout_down(&root, &c);
        no_node_overlap(&l);
        let a_row = 80.0 + c.x_gap + 80.0; // wide subtree's leaf row
        let rigid = a_row + c.x_gap + 20.0; // v1-style width
        assert!(
            l.width < rigid - 1.0,
            "expected tuck: width {} should undercut rigid {}",
            l.width,
            rigid
        );
    }

    #[test]
    fn elbow_edge_spans_root_to_child_anchor() {
        let root = parent(0, 30.0, 20.0, vec![leaf(1, 30.0, 20.0), leaf(2, 30.0, 20.0)]);
        let c = cfg();
        let l = layout_down(&root, &c);
        let e = l.edges.iter().find(|e| e.to == 1).unwrap();
        let r = node_by_id(&l, 0);
        let k = node_by_id(&l, 1);
        let first = e.points[0];
        let last = *e.points.last().unwrap();
        assert!((first.0 - (r.x + r.w / 2.0)).abs() < 1e-9);
        assert!((first.1 - (r.y + r.h)).abs() < 1e-9);
        assert!((last.0 - (k.x + k.w / 2.0)).abs() < 1e-9);
        assert!((last.1 - k.y).abs() < 1e-9);
    }

    #[test]
    fn single_child_trunk_is_straight() {
        let root = parent(0, 30.0, 20.0, vec![leaf(1, 30.0, 20.0)]);
        let l = layout_down(&root, &cfg());
        assert_eq!(l.edges[0].points.len(), 2);
    }

    #[test]
    fn horiz_right_root_centered_over_anchor_span() {
        let branch = parent(0, 40.0, 20.0, vec![leaf(1, 50.0, 20.0), leaf(2, 50.0, 20.0)]);
        let c = cfg();
        let blob = layout_horiz(&branch, &c, false);
        let r = *blob.nodes.iter().find(|n| n.id == 0).unwrap();
        let k1 = *blob.nodes.iter().find(|n| n.id == 1).unwrap();
        let k2 = *blob.nodes.iter().find(|n| n.id == 2).unwrap();
        let span_mid = (k1.y + k1.h / 2.0 + k2.y + k2.h / 2.0) / 2.0;
        assert!((r.y + r.h / 2.0 - span_mid).abs() < 1e-9);
        assert!((k1.x - (40.0 + c.x_gap)).abs() < 1e-9);
    }

    #[test]
    fn horiz_left_mirrors_root_to_right_edge() {
        let branch = parent(0, 40.0, 20.0, vec![leaf(1, 50.0, 20.0)]);
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
        let r = node_by_id(&l, 0);
        assert!((r.y + r.h / 2.0 - l.height / 2.0).abs() < 1e-9);
        assert!((l.width - (30.0 + c.side_gap + 40.0 + c.side_gap + 30.0)).abs() < 1e-9);
    }

    #[test]
    fn mindmap_edge_lands_on_branch_anchor() {
        let root = leaf(0, 40.0, 20.0);
        let branch = parent(1, 30.0, 20.0, vec![leaf(2, 30.0, 20.0)]);
        let c = cfg();
        let l = layout_mindmap(&root, &[], &[branch], &c);
        let e = l.edges.iter().find(|e| e.from == 0 && e.to == 1).unwrap();
        let b1 = node_by_id(&l, 1);
        let last = *e.points.last().unwrap();
        assert!((last.0 - b1.x).abs() < 1e-9);
        assert!((last.1 - (b1.y + b1.h / 2.0)).abs() < 1e-9);
    }

    // --- WBS outline ------------------------------------------------------

    fn sided(id: usize, side: Side, children: Vec<TreeLayoutInput>) -> TreeLayoutInput {
        TreeLayoutInput {
            id,
            size: (60.0, 20.0),
            side,
            children,
        }
    }

    #[test]
    fn zero_size_sibling_keeps_source_order() {
        // A folded phantom collapses to a 0×0 blob; its rects never
        // y-overlap anything, so the separation scan has no
        // constraint — it must still slot AFTER its earlier siblings,
        // not fall back to x = 0.
        let root = parent(
            0,
            40.0,
            20.0,
            vec![leaf(1, 60.0, 20.0), leaf(2, 60.0, 20.0), leaf(3, 0.0, 0.0)],
        );
        let c = cfg();
        let l = layout_down(&root, &c);
        let k2 = node_by_id(&l, 2);
        let ghost = node_by_id(&l, 3);
        assert!(
            ghost.x >= k2.x + k2.w + c.x_gap - 1e-9,
            "zero-size node must sit after its siblings: ghost.x={} k2 right={}",
            ghost.x,
            k2.x + k2.w
        );
    }

    #[test]
    fn wbs_level2_is_horizontal_regardless_of_side() {
        let root = parent(
            0,
            60.0,
            20.0,
            vec![
                sided(1, Side::Left, vec![]),
                sided(2, Side::Right, vec![]),
            ],
        );
        let l = layout_wbs(&root, &cfg());
        let a = node_by_id(&l, 1);
        let b = node_by_id(&l, 2);
        assert!((a.y - b.y).abs() < 1e-9, "level 2 shares one row");
        assert!(a.x < b.x, "source order preserved left to right");
    }

    #[test]
    fn wbs_level3_stacks_vertically_by_side() {
        let root = parent(
            0,
            60.0,
            20.0,
            vec![sided(
                1,
                Side::Right,
                vec![
                    sided(2, Side::Left, vec![]),
                    sided(3, Side::Right, vec![]),
                    sided(4, Side::Right, vec![]),
                ],
            )],
        );
        let c = cfg();
        let l = layout_wbs(&root, &c);
        no_node_overlap(&l);
        let p = node_by_id(&l, 1);
        let trunk_x = p.x + p.w / 2.0;
        let left = node_by_id(&l, 2);
        let r1 = node_by_id(&l, 3);
        let r2 = node_by_id(&l, 4);
        // Left child entirely left of the trunk, right children right.
        assert!(left.x + left.w < trunk_x, "left column left of trunk");
        assert!(r1.x > trunk_x, "right column right of trunk");
        // Columns stack vertically, independent cursors: first row of
        // each column shares the same y.
        assert!((left.y - r1.y).abs() < 1e-9, "first rows align");
        assert!(r2.y > r1.y, "second right row below first");
        // All below the parent.
        assert!(left.y >= p.y + p.h, "outline rows below parent");
    }

    #[test]
    fn wbs_outline_emits_trunk_and_stubs() {
        let root = parent(
            0,
            60.0,
            20.0,
            vec![sided(
                1,
                Side::Right,
                vec![sided(2, Side::Right, vec![]), sided(3, Side::Left, vec![])],
            )],
        );
        let l = layout_wbs(&root, &cfg());
        // Trunk: from == to == parent id, vertical.
        let trunk = l.edges.iter().find(|e| e.from == 1 && e.to == 1).unwrap();
        assert_eq!(trunk.points.len(), 2);
        assert!((trunk.points[0].0 - trunk.points[1].0).abs() < 1e-9, "trunk vertical");
        // Stubs: horizontal, from trunk to each child's inner edge.
        for child in [2usize, 3] {
            let stub = l.edges.iter().find(|e| e.from == 1 && e.to == child).unwrap();
            assert_eq!(stub.points.len(), 2);
            assert!((stub.points[0].1 - stub.points[1].1).abs() < 1e-9, "stub horizontal");
            let cnode = node_by_id(&l, child);
            let inner = if child == 3 { cnode.x + cnode.w } else { cnode.x };
            assert!((stub.points[1].0 - inner).abs() < 1e-9, "stub reaches box edge");
        }
    }

    #[test]
    fn wbs_nested_outline_indents_and_avoids_overlap() {
        // A deep left child whose own (right-side) subtree pokes back
        // toward the parent trunk must not collide with the right
        // column.
        let deep_left = sided(
            2,
            Side::Left,
            vec![sided(3, Side::Right, vec![sided(4, Side::Right, vec![])])],
        );
        let root = parent(
            0,
            60.0,
            20.0,
            vec![sided(1, Side::Right, vec![deep_left, sided(5, Side::Right, vec![])])],
        );
        let l = layout_wbs(&root, &cfg());
        no_node_overlap(&l);
    }
}
