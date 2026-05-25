//! Sequence diagram IR (the `Raw` / `Structured` flavors).

use super::common::{NotePosition, Skinparam};

#[derive(Clone, Debug)]
pub enum SequenceDiagram {
    Raw {
        name: Option<String>,
        title: Option<String>,
        body: String,
        hints: SequenceHints,
    },
    Structured(StructuredSequence),
}

#[derive(Clone, Debug, Default)]
pub struct SequenceHints {
    /// Number of declared participants (or, if none are declared,
    /// distinct endpoints implied by arrow lines, clamped to a minimum).
    pub participants: u32,
    /// Longest message label seen (in characters), used to pad the
    /// codegen width estimate when labels are unusually long.
    pub max_label_chars: u32,
}

#[derive(Clone, Debug, Default)]
pub struct StructuredSequence {
    pub name: Option<String>,
    pub title: Option<String>,
    pub participants: Vec<Participant>,
    pub steps: Vec<Step>,
    pub skinparams: Vec<Skinparam>,
    /// PlantUML's `autoactivate on` toggle. When true, codegen emits the
    /// directive so blockcell auto-derives activations from call/return pairs.
    pub autoactivate: bool,
}

#[derive(Clone, Debug)]
pub struct Participant {
    pub kind: ParticipantKind,
    /// Canonical identifier used by messages — either the explicit alias
    /// (`as Foo`) or the bare display label.
    pub id: String,
    /// User-visible label. Equal to `id` when no `as` clause was given.
    pub display: String,
    /// Raw body lines from a `participant Foo [ ... ]` block (PlantUML's
    /// rich-content syntax with `=Title`, `----`, `""text""`, etc.). When
    /// present, codegen re-emits the bracketed form so blockcell can render
    /// the formatted label; `display` is left as the bare id for fallback.
    pub display_block: Option<Vec<String>>,
    /// Raw color spec from the participant line (e.g. `"#LightBlue"`),
    /// preserved so codegen can re-emit it for `seq-puml` to parse.
    pub color: Option<String>,
    pub line: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ParticipantKind {
    Participant,
    Actor,
    Boundary,
    Control,
    Entity,
    Database,
    Collections,
    Queue,
}

impl ParticipantKind {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Participant => "participant",
            Self::Actor => "actor",
            Self::Boundary => "boundary",
            Self::Control => "control",
            Self::Entity => "entity",
            Self::Database => "database",
            Self::Collections => "collections",
            Self::Queue => "queue",
        }
    }

    pub fn from_keyword(kw: &str) -> Option<Self> {
        Some(match kw {
            "participant" => Self::Participant,
            "actor" => Self::Actor,
            "boundary" => Self::Boundary,
            "control" => Self::Control,
            "entity" => Self::Entity,
            "database" => Self::Database,
            "collections" => Self::Collections,
            "queue" => Self::Queue,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub enum Step {
    Message {
        from: String,
        to: String,
        /// Raw arrow text — `"->"`, `"-->"`, `"->>"`, `"-[#red]->"`, etc.
        /// Kept verbatim so the rendered arrow style matches PUML exactly
        /// once `seq-puml` re-parses it.
        arrow: String,
        label: Option<String>,
        line: usize,
    },
    Note {
        position: NotePosition,
        participants: Vec<String>,
        text: String,
        line: usize,
    },
    Divider {
        label: String,
        line: usize,
    },
    /// Raw `autonumber [...]` directive. Re-emitted as-is to seq-puml.
    Autonumber {
        raw: String,
        line: usize,
    },
    Activate {
        participant: String,
        color: Option<String>,
        line: usize,
    },
    Deactivate {
        participant: String,
        line: usize,
    },
    /// `create [participant] X` — declares the participant lazily.
    Create(Participant),
    Destroy {
        participant: String,
        line: usize,
    },
    /// `return [label]` — sugar for an arrow back to the caller.
    Return {
        label: Option<String>,
        line: usize,
    },
    Fragment {
        kind: FragmentKind,
        label: Option<String>,
        branches: Vec<Branch>,
        line: usize,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Branch {
    /// Label on the opening keyword (`alt cond`, `else other`, `loop while`).
    pub label: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FragmentKind {
    Alt,
    Opt,
    Loop,
    Par,
    Group,
    Break,
    Critical,
}

impl FragmentKind {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Alt => "alt",
            Self::Opt => "opt",
            Self::Loop => "loop",
            Self::Par => "par",
            Self::Group => "group",
            Self::Break => "break",
            Self::Critical => "critical",
        }
    }

    pub fn from_keyword(kw: &str) -> Option<Self> {
        Some(match kw {
            "alt" => Self::Alt,
            "opt" => Self::Opt,
            "loop" => Self::Loop,
            "par" => Self::Par,
            "group" => Self::Group,
            "break" => Self::Break,
            "critical" => Self::Critical,
            _ => return None,
        })
    }

    /// Whether this fragment kind supports `else` branches.
    pub fn has_else(self) -> bool {
        matches!(self, Self::Alt | Self::Critical)
    }
}
