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

/// Mind-map diagram (`@startmindmap`). Same `TreeNode` shape as WBS; codegen
/// classifies each first-level child by its [`NodeSide`] and emits a
/// `mindmap(root, lefts: (...), rights: (...))` call. Deeper levels stay in
/// the chosen direction via `tree.typ`'s direction-state inheritance.
#[derive(Clone, Debug)]
pub struct MindMapDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    pub root: TreeNode,
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
    /// PlantUML's `_` modifier — single underline, no fill / no border.
    /// v1 codegen still emits a default node here; the M3 painter will add
    /// the `"underline"` shape variant to `node()`.
    Line,
}
