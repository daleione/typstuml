//! Font-scaled spacing table for the compound Sugiyama layout.
//!
//! Values are ported from `@markdown-viewer/draw-uml`'s ELK recipe
//! (`elk-adapter.ts::elkSpacing` + `shared/theme.ts`), which derives
//! every spacing constant from `fontSize * n/12` at a 12px reference
//! font. `for_font` applies the same ratio at this project's font size
//! so the whole spacing system scales together. See
//! `docs/cuca-architecture-layout-redesign.md` §3.1 for the mapping
//! table and rationale.
//!
//! `VisualGraph` carries one `Spacing` (default [`Spacing::legacy`]),
//! read by cuca's compound layout, `hierarchy::apply_cluster_margins`,
//! and `tighten`'s sibling/stranger separation. Every other diagram
//! family (record graphs, state, sequence, …) never changes it, so
//! this table is a no-op outside the cuca hierarchical layout path.

#[derive(Debug, Clone, Copy)]
pub struct Spacing {
    /// Gap between sibling nodes within the same rank (ELK
    /// `spacing.nodeNode`).
    pub node_node: f64,
    /// Reduced sibling gap for root-level (unclustered) nodes when the
    /// diagram has containers — packages already carry their own
    /// padding, so root-level gaps can be tighter (ELK's
    /// `groupNodeNode`, applied at the root when groups are present).
    pub root_node_node: f64,
    /// Gap between ranks (ELK `layered.spacing.nodeNodeBetweenLayers`).
    pub between_layers: f64,
    /// Gap between parallel edges (ELK `spacing.edgeEdge`).
    pub edge_edge: f64,
    /// Gap between an edge and a node it passes (ELK
    /// `spacing.edgeNode`).
    pub edge_node: f64,
    /// Gap between an edge and its label (ELK `spacing.edgeLabel`).
    pub edge_label: f64,
    /// Padding between a container's outer rectangle and its inner
    /// content (ELK cluster padding, left/right/bottom side).
    pub cluster_pad: f64,
    /// Extra padding folded into a container's label band on top of
    /// the measured label height (ELK cluster padding's larger top
    /// side, which also reserves room for the label/tab).
    pub cluster_label_extra: f64,
    /// Minimum gap enforced between sibling cluster frames / between a
    /// cluster frame and any node outside it.
    pub cluster_gap: f64,
    /// Rounded-corner radius for orthogonal edge routing (M3).
    pub ortho_arc: f64,
    /// Minimum gap between parallel orthogonal edge trunks (M3/M4's
    /// `separate_overlapping`).
    pub ortho_min_gap: f64,
}

impl Spacing {
    /// ELK-recipe values scaled to `font_pt` from the 12px reference
    /// the upstream constants were measured at.
    pub fn for_font(font_pt: f64) -> Self {
        let em = font_pt / 12.0;
        Spacing {
            node_node: 20.0 * em,
            root_node_node: 10.0 * em,
            between_layers: 40.0 * em,
            edge_edge: 10.0 * em,
            edge_node: 20.0 * em,
            edge_label: 5.0 * em,
            cluster_pad: 30.0 * em,
            cluster_label_extra: 10.0 * em,
            // ELK's inter-group gap doubles as the frame-to-frame /
            // frame-to-stranger minimum gap in our post-hoc tighten
            // pass.
            cluster_gap: 10.0 * em,
            ortho_arc: 10.0 * em,
            ortho_min_gap: 5.0 * em,
        }
    }

    /// Pre-M2 constants, verbatim — the default for every diagram
    /// family that never opts into the ELK recipe.
    pub fn legacy() -> Self {
        Spacing {
            node_node: 6.0,
            root_node_node: 6.0,
            between_layers: 12.0,
            edge_edge: 6.0,
            edge_node: 6.0,
            edge_label: 6.0,
            cluster_pad: 14.0,
            cluster_label_extra: 14.0,
            cluster_gap: 12.0,
            ortho_arc: 8.0,
            ortho_min_gap: 5.0,
        }
    }
}

impl Default for Spacing {
    fn default() -> Self {
        Self::legacy()
    }
}
