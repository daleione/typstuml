//! State diagram IR (PlantUML `@startuml` with `[*]` / `state` decls).
//!
//! A flat node table + transition list + side tables for nesting
//! (`StateNode.parent` / `.children`) and concurrent regions
//! (`RegionGroup`). See `docs/state-diagram-design.md`.

use super::common::{Direction, LayoutDirection, LineStyle, NotePosition, Skinparam};

#[derive(Clone, Debug, Default)]
pub struct StateDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    /// All states, in declaration order — simple states, composite shells,
    /// and pseudostates (initial / final / choice / fork / join / history /
    /// synchro bar). Nesting is expressed via `StateNode.parent`.
    pub nodes: Vec<StateNode>,
    pub transitions: Vec<Transition>,
    /// Concurrent partitions inside composite states. One entry per
    /// composite that actually has a `--` / `||` divider; composites
    /// without a divider don't appear here (painter renders them as a
    /// single region).
    pub regions: Vec<RegionGroup>,
    pub notes: Vec<StateNote>,
    pub skinparams: Vec<Skinparam>,
    pub hide_empty_description: bool,
    pub direction: LayoutDirection,
}

#[derive(Clone, Debug)]
pub struct StateNode {
    /// Canonical id used by transitions — alias when `state "X" as Foo`,
    /// else the display text.
    pub id: String,
    pub display: String,
    pub kind: StateKind,
    /// `entry / exit / do_action()` body rows, in declaration order. Each
    /// row has leading/trailing whitespace stripped; codegen renders them
    /// verbatim under a divider line.
    pub body: Vec<String>,
    /// Raw color spec (`#LightBlue`, `#ABC`). Codegen normalizes.
    pub fill: Option<String>,
    pub border_style: Option<BorderStyle>,
    /// Raw border color spec from `##[style]color` / `##color`.
    pub border_color: Option<String>,
    pub stereotype: Option<String>,
    /// Child node ids — non-empty only for `kind == Composite`.
    pub children: Vec<String>,
    /// Parent composite id; `None` for top-level nodes.
    pub parent: Option<String>,
    pub line: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StateKind {
    /// Simple state — rounded rectangle, optional `entry/exit/do` body.
    Simple,
    /// Composite state — rounded rectangle outer frame + nested subgraph.
    Composite,
    /// `[*]` on the left of an arrow, or `<<start>>` — small filled circle.
    Initial,
    /// `[*]` on the right of an arrow, or `<<end>>` — ringed filled circle.
    Final,
    /// `<<choice>>` — diamond.
    Choice,
    /// `<<fork>>` — solid bar.
    Fork,
    /// `<<join>>` — solid bar; visual identical to `Fork`, kept distinct
    /// so future overlays can differentiate.
    Join,
    /// `[H]` / `<<history>>` — circle with "H".
    History,
    /// `[H*]` / `<<history*>>` — circle with "H*".
    DeepHistory,
    /// `==Name==` — short thick bar synchronizing concurrent regions.
    SynchroBar,
    /// `<<entryPoint>>` — hollow circle on a composite's border.
    EntryPoint,
    /// `<<exitPoint>>` — hollow circle with an X on a composite's border.
    ExitPoint,
}

impl StateKind {
    /// Whether the node is a pseudostate (fixed-size glyph) rather than a
    /// text-bearing rounded rectangle.
    pub fn is_pseudo(self) -> bool {
        !matches!(self, Self::Simple | Self::Composite)
    }

    /// Keyword written into the painter's `kind:` slot.
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Composite => "composite",
            Self::Initial => "initial",
            Self::Final => "final",
            Self::Choice => "choice",
            Self::Fork => "fork",
            Self::Join => "join",
            Self::History => "history",
            Self::DeepHistory => "deep-history",
            Self::SynchroBar => "synchro-bar",
            Self::EntryPoint => "entry-point",
            Self::ExitPoint => "exit-point",
        }
    }

    /// Map a PlantUML stereotype (case-insensitive, angle brackets
    /// stripped) to a pseudostate kind. Returns `None` for an ordinary
    /// stereotype that doesn't pick a shape.
    pub fn from_stereotype(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "choice" => Some(Self::Choice),
            "fork" => Some(Self::Fork),
            "join" => Some(Self::Join),
            "start" => Some(Self::Initial),
            "end" => Some(Self::Final),
            "history" => Some(Self::History),
            "history*" => Some(Self::DeepHistory),
            "entrypoint" => Some(Self::EntryPoint),
            "exitpoint" => Some(Self::ExitPoint),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub from: String,
    pub to: String,
    /// `event [guard] / action` — each of the three parts is optional.
    pub event: Option<String>,
    pub guard: Option<String>,
    pub action: Option<String>,
    pub line_style: LineStyle,
    /// Raw color spec from `-[#color]->`.
    pub color: Option<String>,
    /// User-supplied direction hint (`-up->`, `-l->`).
    pub direction: Option<Direction>,
    /// PlantUML treats a single-dash arrow (`A -> B`) or a left/right
    /// direction hint as a **horizontal** link: source and target sit on
    /// the same layout rank, side by side. A double-dash arrow (`A --> B`)
    /// or an up/down hint is a **vertical** rank edge. Codegen keeps
    /// horizontal transitions out of the Sugiyama rank graph so they
    /// don't push the target down a rank.
    pub horizontal: bool,
    /// Minimum rank span (dot's `minlen`): PlantUML maps the dash count to
    /// `minlen = dashes − 1`, so `-->` = 1, `--->` = 2, … The target sits
    /// at least this many ranks below the source. 1 for horizontal links
    /// (rank handled separately). Default 1.
    pub min_rank: usize,
    pub line: usize,
}

/// Concurrent partitions inside one composite state. `partitions[i]` is
/// the list of node ids in region `i`, in declaration order.
#[derive(Clone, Debug)]
pub struct RegionGroup {
    pub composite_id: String,
    pub orientation: RegionOrient,
    pub partitions: Vec<Vec<String>>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RegionOrient {
    /// `--` divider — regions stacked top/bottom, horizontal divider line.
    Horizontal,
    /// `||` divider — regions side by side, vertical divider line.
    Vertical,
}

#[derive(Clone, Debug)]
pub struct StateNote {
    pub anchor: NoteAnchor,
    pub body: String,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub enum NoteAnchor {
    /// `note left of Foo` / `note right of Foo`.
    OfNode { node_id: String, side: NotePosition },
    /// `note on link` — bound to a transition by index into
    /// [`StateDiagram::transitions`].
    OnLink { transition_idx: usize },
    /// `note "..." as Nx` — a standalone note. `links` holds the state
    /// ids it was connected to via `Nx .. State` lines (empty if the
    /// note was never connected).
    Floating { id: String, links: Vec<String> },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BorderStyle {
    Solid,
    Dashed,
    Bold,
    Dotted,
}
