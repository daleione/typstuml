//! Activity diagram IR (PlantUML `activitydiagram3` / new syntax).
//!
//! The IR is a **statement tree** — order is encoded in tree position, no
//! edges. See `docs/activity-diagram-design.md` for the painter contract.

use super::common::{LayoutDirection, NotePosition, Skinparam};

#[derive(Clone, Debug, Default)]
pub struct ActivityDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    /// Top-level statement sequence. Implicitly wrapped in a `flow-col`
    /// by codegen.
    pub body: Vec<ActivityStmt>,
    pub skinparams: Vec<Skinparam>,
    /// `left to right direction` — captured but ignored by M0 codegen.
    pub direction: LayoutDirection,
}

#[derive(Clone, Debug)]
pub enum ActivityStmt {
    /// `:label;` — a plain action. `label` carries the (possibly
    /// multi-line) text already split on `\n`. `kind` distinguishes
    /// stereotype-driven shape variants (`<<input>>` / `<<output>>` /
    /// etc.); M0 codegen renders them all as rectangles.
    Action {
        label: Vec<String>,
        kind: ActionKind,
        color: Option<String>,
        url: Option<String>,
        notes: Vec<NoteAttach>,
        /// Label on the incoming edge (`-> foo;` right before this
        /// action). M0 codegen renders this on the auto-inserted
        /// arrow via `flow-col`'s `edge-label` slot.
        edge_label: Option<String>,
        line: usize,
    },
    Start {
        line: usize,
    },
    Stop {
        line: usize,
    },
    /// `end` — distinct from `stop` so codegen can choose a different
    /// glyph (PlantUML draws the abort `⊗`).
    End {
        line: usize,
    },
    /// `detach` / `kill` — PlantUML visualises both with the same `⊥`
    /// tee. Merged at parse time.
    Detach {
        line: usize,
    },
    /// `break` — early loop exit. M0 records and drops.
    Break {
        line: usize,
    },
    /// `label NAME` — a `goto` jump target. M0 records and drops.
    GotoLabel {
        name: String,
        line: usize,
    },
    /// `goto NAME` — unconditional jump. M0 records and drops.
    Goto {
        name: String,
        line: usize,
    },
    If {
        cond: String,
        then_label: Option<String>,
        then_branch: Vec<ActivityStmt>,
        elseifs: Vec<ElseIfBranch>,
        else_label: Option<String>,
        else_branch: Option<Vec<ActivityStmt>>,
        line: usize,
    },
    /// `repeat … repeat while (cond) [is (yes) not (no)]`.
    Repeat {
        body: Vec<ActivityStmt>,
        /// `backward :label;` block — the reverse-direction action that
        /// piggybacks on the back-edge. M0 codegen ignores.
        backward: Option<Vec<ActivityStmt>>,
        /// `None` for an unconditional `repeat` (rare; usually paired
        /// with `break`).
        cond: Option<String>,
        is_label: Option<String>,
        not_label: Option<String>,
        line: usize,
    },
    /// `while (cond) [is (yes)] … endwhile [(no)]`.
    While {
        cond: String,
        is_label: Option<String>,
        not_label: Option<String>,
        body: Vec<ActivityStmt>,
        line: usize,
    },
    /// `fork … fork again … end fork [merge|no merge]`. Concurrent.
    Fork {
        branches: Vec<Vec<ActivityStmt>>,
        merge: bool,
        line: usize,
    },
    /// `split … split again … end split`. Visually identical to fork;
    /// kept as a separate variant so future stereotype overlays can
    /// differentiate.
    Split {
        branches: Vec<Vec<ActivityStmt>>,
        merge: bool,
        line: usize,
    },
    Switch {
        cond: String,
        cases: Vec<SwitchCase>,
        line: usize,
    },
    /// `partition Name [#color] { … }` (also accepts `package` /
    /// `rectangle` / `card` / `group` per PlantUML's CommandPartition3).
    Partition {
        kind: PartitionKind,
        label: String,
        color: Option<String>,
        body: Vec<ActivityStmt>,
        line: usize,
    },
    /// `|Lane|` / `|#color| Lane |` — switch the active swimlane.
    /// Infix syntax, not a block. M0 codegen drops; M2 groups statements
    /// between adjacent `SwimlaneSwitch`es into lanes.
    SwimlaneSwitch {
        label: String,
        color: Option<String>,
        line: usize,
    },
}

/// Shape selector for `:label;` actions, mirroring PlantUML's `BoxStyle`
/// enum (see `activitydiagram3/ftile/BoxStyle.java`). M0 codegen renders
/// every variant as a plain rectangle; richer painters land later.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ActionKind {
    #[default]
    Rectangle,
    Input,
    Output,
    SendSignal,
    ReceiveSignal,
    Procedure,
    Load,
    Save,
    Continuous,
    Task,
    Object,
    ObjectSignal,
    Trigger,
    AcceptEvent,
    TimeEvent,
}

impl ActionKind {
    /// Map a PlantUML stereotype keyword (case-insensitive) to its
    /// shape variant. Returns `Rectangle` for unknown / missing.
    pub fn from_stereotype(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "input" => Self::Input,
            "output" => Self::Output,
            "sendsignal" | "send-signal" => Self::SendSignal,
            "receivesignal" | "acceptevent" | "accept-event" => Self::ReceiveSignal,
            "procedure" => Self::Procedure,
            "load" => Self::Load,
            "save" => Self::Save,
            "continuous" => Self::Continuous,
            "task" => Self::Task,
            "object" => Self::Object,
            "objectsignal" | "object-signal" => Self::ObjectSignal,
            "trigger" => Self::Trigger,
            "timeevent" | "time-event" => Self::TimeEvent,
            _ => Self::Rectangle,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PartitionKind {
    Partition,
    Package,
    Rectangle,
    Card,
    Group,
}

#[derive(Clone, Debug)]
pub struct ElseIfBranch {
    pub cond: String,
    /// `elseif (c) then (label)` — the `(label)` portion.
    pub label: Option<String>,
    pub branch: Vec<ActivityStmt>,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct SwitchCase {
    /// `case (value)` — body of the parentheses.
    pub value: String,
    pub branch: Vec<ActivityStmt>,
    pub line: usize,
}

/// Note attached to the *previous* action in a statement list. Position is
/// always `LeftOf` / `RightOf` in activity (PlantUML doesn't expose
/// `over` here); we reuse [`NotePosition`] from the sequence IR.
#[derive(Clone, Debug)]
pub struct NoteAttach {
    pub position: NotePosition,
    pub text: Vec<String>,
    pub line: usize,
}
