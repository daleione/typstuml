//! Intermediate representation.
//!
//! The IR is the contract between parsers and codegen. Long-term goal is a
//! single shared "data → render" protocol so codegen never depends on
//! parser-specific shapes.
//!
//! Sequence diagrams ship in two flavors:
//!   - [`SequenceDiagram::Raw`] — body kept verbatim, parser does only a
//!     light hint scan; rendering is fully delegated to `blockcell.seq-puml`.
//!     Reserved as a bypass for future loose-mode error recovery; the
//!     current parser doesn't produce it.
//!   - [`SequenceDiagram::Structured`] — full AST built by the native Rust
//!     parser, with line-accurate metadata for diagnostics and a place for
//!     `skinparam` values to live before codegen translates them.

#[derive(Clone, Debug)]
pub struct Document {
    pub diagrams: Vec<Diagram>,
}

#[derive(Clone, Debug)]
pub enum Diagram {
    Sequence(SequenceDiagram),
    Json(JsonDiagram),
    Yaml(YamlDiagram),
    Wbs(WbsDiagram),
    MindMap(MindMapDiagram),
    Class(ClassDiagram),
    // Future: State(StateDiagram), Activity(...), ...
}

#[derive(Clone, Debug)]
pub struct JsonDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    /// Parsed JSON value. The full serde_json::Value tree is the AST — there's
    /// no further normalization since `tree` codegen walks it recursively.
    pub root: serde_json::Value,
}

/// YAML diagram. Parsed via `serde_yaml_ng` directly into a
/// `serde_json::Value` so it can share the JSON record-graph codegen path —
/// the rendered output for an equivalent JSON / YAML document is identical.
#[derive(Clone, Debug)]
pub struct YamlDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    pub root: serde_json::Value,
}

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
}

#[derive(Clone, Debug)]
pub struct Participant {
    pub kind: ParticipantKind,
    /// Canonical identifier used by messages — either the explicit alias
    /// (`as Foo`) or the bare display label.
    pub id: String,
    /// User-visible label. Equal to `id` when no `as` clause was given.
    pub display: String,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NotePosition {
    Over,
    LeftOf,
    RightOf,
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

#[derive(Clone, Debug)]
pub struct Skinparam {
    pub key: String,
    pub value: String,
    pub line: usize,
}

/// Class diagram (`@startuml` with `class` / `interface` / `abstract` /
/// `enum` / `package` / `namespace` declarations). Field naming
/// (`Entity`/`Relation`/`Container`) is deliberately neutral — the same
/// shape will host object and component diagrams once they land.
///
/// `containers` carries `package` / `namespace` / `together` /
/// `folder` / `frame` / `node` / `cloud` blocks; codegen consumes
/// them via the compound layout pass.
#[derive(Clone, Debug, Default)]
pub struct ClassDiagram {
    pub name: Option<String>,
    pub title: Option<String>,
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
    pub containers: Vec<Container>,
    pub skinparams: Vec<Skinparam>,
    pub hide: HideOptions,
    pub direction: LayoutDirection,
}

/// Top-level layout flow. PlantUML's `left to right direction` flips
/// the Sugiyama orientation; everything else (`top to bottom`,
/// default) keeps TB.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LayoutDirection {
    #[default]
    TopToBottom,
    LeftToRight,
}

/// `hide …` / `show …` global filters from PlantUML. Renderers consult
/// these before laying out members or drawing the marker / stereotype
/// chip. Per-class / per-stereotype scoping is not implemented.
#[derive(Copy, Clone, Debug, Default)]
pub struct HideOptions {
    /// `hide circle` — suppresses the C/I/A/E marker chip on every
    /// entity.
    pub circle: bool,
    /// `hide stereotype` — drops the `<<…>>` line above the name.
    pub stereotype: bool,
    /// `hide members` — drops both fields and methods compartments.
    pub members: bool,
    /// `hide methods` — drops only the methods compartment.
    pub methods: bool,
    /// `hide fields` / `hide attributes` — drops only the fields
    /// compartment.
    pub fields: bool,
}

#[derive(Clone, Debug)]
pub struct Entity {
    pub kind: EntityKind,
    /// Canonical id used by relations — alias if `class A as Foo`, else
    /// `display`.
    pub id: String,
    pub display: String,
    /// Generic parameters as written between `<` and `>` (e.g. `T, U`).
    pub generic: Option<String>,
    /// `<<...>>` text without the angle brackets and without the
    /// `(L, color)` custom-marker prefix (which goes into
    /// `stereotype_marker`).
    pub stereotype: Option<String>,
    /// Custom marker `(letter, color)` from `<<(L, #color) text>>`. The
    /// letter overrides the kind-default chip glyph; the color is a
    /// raw color spec (`#ABCDEF`, `red`, …) or `None` for the kind
    /// default.
    pub stereotype_marker: Option<(String, Option<String>)>,
    pub fields: Vec<Member>,
    pub methods: Vec<Member>,
    /// Free-text body. Set only for [`EntityKind::Note`]; renderers use
    /// it instead of `fields` / `methods`. May contain `\n`.
    pub body: Option<String>,
    /// Raw color spec (`#LightBlue`, `#ABC`). Codegen normalizes.
    pub fill: Option<String>,
    pub line: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Class,
    Interface,
    Abstract,
    Enum,
    Annotation,
    Struct,
    Exception,
    Protocol,
    EntityShape,
    Circle,
    Diamond,
    /// Free-text annotation rendered as a yellow dog-eared sticky note.
    /// Created from PlantUML `note` syntax; participates in layout like
    /// any other entity.
    Note,
}

impl EntityKind {
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Abstract => "abstract",
            Self::Enum => "enum",
            Self::Annotation => "annotation",
            Self::Struct => "struct",
            Self::Exception => "exception",
            Self::Protocol => "protocol",
            Self::EntityShape => "entity",
            Self::Circle => "circle",
            Self::Diamond => "diamond",
            Self::Note => "note",
        }
    }

    pub fn from_keyword(kw: &str) -> Option<Self> {
        Some(match kw {
            "class" => Self::Class,
            "interface" => Self::Interface,
            "abstract" => Self::Abstract,
            "enum" => Self::Enum,
            "annotation" => Self::Annotation,
            "struct" => Self::Struct,
            "exception" => Self::Exception,
            "protocol" => Self::Protocol,
            "entity" => Self::EntityShape,
            "circle" => Self::Circle,
            "diamond" => Self::Diamond,
            "note" => Self::Note,
            _ => return None,
        })
    }

    /// Single-letter glyph for the stereotype circle (`C` / `I` / `A` / `E`).
    /// `None` for shapes that don't get a marker (circle / diamond / note).
    pub fn marker_letter(self) -> Option<char> {
        Some(match self {
            Self::Class | Self::Struct | Self::Exception => 'C',
            Self::Interface | Self::Protocol => 'I',
            Self::Abstract => 'A',
            Self::Enum => 'E',
            Self::Annotation => '@',
            Self::EntityShape => 'E',
            Self::Circle | Self::Diamond | Self::Note => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Member {
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_abstract: bool,
    /// Body after stripping the leading visibility character and any
    /// `{static}` / `{abstract}` modifier — e.g. `getName(): String`.
    /// Codegen renders verbatim with markup escaping.
    pub body: String,
    pub line: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
    Protected,
    Package,
    None,
}

impl Visibility {
    /// Single-glyph prefix used by codegen (matches PlantUML's default).
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Public => "+",
            Self::Private => "-",
            Self::Protected => "#",
            Self::Package => "~",
            Self::None => "",
        }
    }

    pub fn from_char(c: char) -> Option<Self> {
        Some(match c {
            '+' => Self::Public,
            '-' => Self::Private,
            '#' => Self::Protected,
            '~' => Self::Package,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Relation {
    pub from: String,
    pub to: String,
    /// PlantUML's couple-link / association-class syntax. When set,
    /// `from_couple = Some((A, B))` and `from = ""` — the edge starts
    /// at the midpoint of the existing relation between A and B and
    /// runs to `to`. Codegen renders it as a dashed connector so an
    /// association class can attach to the link between two classes.
    pub from_couple: Option<(String, String)>,
    /// `A::field` port: when set, the edge anchors at the named member
    /// compartment row of `from` instead of the entity's bottom-mid.
    /// Codegen does not yet shift the anchor — the field is captured
    /// to keep the parser's auto-create logic from inventing a phantom
    /// entity named `A::field`.
    pub from_port: Option<String>,
    pub to_port: Option<String>,
    /// Decoration on the `from` side of the link (e.g. the `<|` in `<|--`).
    pub head_from: ArrowHead,
    /// Decoration on the `to` side (the `|>` in `--|>`).
    pub head_to: ArrowHead,
    pub line_style: LineStyle,
    /// User-supplied direction hint (`-up->`, `-left->`); codegen may use
    /// it to bias Sugiyama orientation but isn't required to honour it.
    pub direction: Option<Direction>,
    pub label: Option<String>,
    pub mult_from: Option<String>,
    pub mult_to: Option<String>,
    pub role_from: Option<String>,
    pub role_to: Option<String>,
    pub stereotype: Option<String>,
    pub color: Option<String>,
    /// Body of a `note on link` attached to this relation. Codegen
    /// renders a yellow sticky next to the edge midpoint.
    pub note: Option<String>,
    pub line: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ArrowHead {
    None,
    /// `<|` / `|>` — generalization (extends).
    TriangleOpen,
    /// `<` / `>` — directed association.
    ArrowOpen,
    /// `o` — aggregation.
    DiamondOpen,
    /// `*` — composition.
    DiamondFilled,
    /// `x` — non-navigable.
    Cross,
    /// `+` — private internal.
    Plus,
    /// `(0` / `0)` / `(0)` — middle circle / lollipop variant.
    CircleConnect,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LineStyle {
    Solid,
    Dashed,
    Dotted,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Container for `package { }` / `namespace { }` / `together { }` etc.
/// Codegen's compound layout pass runs a sub-Sugiyama per cluster and
/// nests them under a super-Sugiyama; cluster bboxes are emitted as
/// `packages: (...)` for the painter to draw.
#[derive(Clone, Debug)]
pub struct Container {
    pub kind: ContainerKind,
    pub label: String,
    pub stereotype: Option<String>,
    pub children_entities: Vec<String>,
    pub children_containers: Vec<usize>,
    pub line: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Package,
    Namespace,
    Folder,
    Frame,
    Cloud,
    Node,
    Together,
}
