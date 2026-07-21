//! WBS / mind-map diagrams — a rooted `TreeNode` tree.

/// Work-Breakdown-Structure diagram (`@startwbs`). The IR is a single rooted
/// tree of [`TreeNode`]s; codegen flattens it into a nested
/// `tree(node[…], …)` Typst expression rendered by `blockcell`'s
/// `tree.typ` painter. Mind maps reuse [`TreeNode`] — they differ from WBS
/// only in the active [`NodeSide`] values and the chosen painter entry
/// point ([`MindMapDiagram`] uses `mindmap`).
#[derive(Clone, Debug)]
pub struct WbsDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    pub root: TreeNode,
}

/// Mind-map diagram (`@startmindmap`). Same `TreeNode` shape as WBS.
/// Codegen classifies each root's first-level children by [`NodeSide`]
/// and lays the two columns out around the central root.
///
/// `roots`: PlantUML allows several depth-1 nodes in one block; each
/// becomes its own mind map, stacked vertically in the render (always
/// at least one entry). `direction` mirrors the `top to bottom
/// direction` directive — the whole diagram transposes, so `left`-side
/// branches grow upward and `right`-side branches grow downward.
#[derive(Clone, Debug)]
pub struct MindMapDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    pub roots: Vec<TreeNode>,
    pub direction: MapDirection,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum MapDirection {
    #[default]
    LeftToRight,
    TopToBottom,
}

#[derive(Clone, Debug)]
pub struct TreeNode {
    /// One entry per source line. Multi-line labels (the `:line1\nline2;`
    /// command form) preserve their split. Codegen joins with Typst's hard
    /// line break inside the painter's content slot.
    pub label: Vec<String>,
    /// `Default` for plain `*+-` markers. WBS optionally accepts `<` / `>`
    /// after the marker and stores them here; v1 codegen ignores the side
    /// (renders all children below the parent) but the IR keeps it so the
    /// M2 direction-aware `tree()` painter can pick it up without an IR
    /// migration.
    pub side: NodeSide,
    pub shape: NodeShape,
    /// Raw `[#color]` spec — `"#FF0000"`, `"#red"`, etc. Codegen translates
    /// hex forms to `rgb("#…")`; named-color resolution waits for the P0.3
    /// shared color-spec parser.
    pub fill: Option<String>,
    /// Optional `(code)` or `as code` alias. v1 keeps it for round-tripping;
    /// no cross-node referencing is supported yet.
    pub id: Option<String>,
    /// 1-based source line of the marker that introduced this node.
    pub line: usize,
    pub children: Vec<TreeNode>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeSide {
    Default,
    Left,
    Right,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeShape {
    /// Default filled rounded box.
    Box,
    /// PlantUML's `_` modifier — "remove the box drawing": bare text,
    /// no border, no fill (an explicit `[#color]` is ignored, matching
    /// PlantUML). Codegen maps this to the painter's `"plain"` shape.
    Line,
    /// PlantUML's `_` **without a label** (WBS "skipping a layer"):
    /// the node is removed completely — zero size, never painted — and
    /// its children hang off a trunk dropped straight from the point
    /// where the node would have been, visually reporting to the
    /// grandparent.
    Phantom,
}
