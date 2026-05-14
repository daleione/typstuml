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
//!
//! Cuca diagrams (the class / component / deployment / use case family —
//! see `docs/cuca-diagram-design.md`) live in [`CucaDiagram`]. The shape
//! of each entity is selected by the [`USymbol`] enum (shared with
//! containers, mirroring PlantUML's `USymbols.java` registry), and the
//! shape-specific extras (class members, note body, object fields)
//! live in [`EntityKindData`]. The single [`Diagram::Cuca`] variant
//! covers what PlantUML internally calls `class`, `description`
//! (component / deployment / use case), and `object` diagrams.

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
    Cuca(CucaDiagram),
    Activity(ActivityDiagram),
    State(StateDiagram),
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

/// Cuca diagram — covers PlantUML's class / component / deployment /
/// use case / object families with one IR. The shape of each entity is
/// chosen by its [`USymbol`]; class-family extras (fields / methods /
/// generic) live in [`EntityKindData::Compartment`]. See
/// `docs/cuca-diagram-design.md` for the design rationale and
/// PlantUML-side references.
#[derive(Clone, Debug, Default)]
pub struct CucaDiagram {
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
    /// Visual shape selector. `USymbol::None` means "class-family
    /// compartment box" — the default for `class`, `interface`,
    /// `abstract`, etc. Other variants pick a desc-family shape
    /// (`Component`, `Database`, `Cloud`, `Actor`, …).
    pub usymbol: USymbol,
    /// Canonical id used by relations — alias if `class A as Foo`, else
    /// `display`.
    pub id: String,
    pub display: String,
    /// `<<...>>` text without the angle brackets and without the
    /// `(L, color)` custom-marker prefix (which goes into
    /// `stereotype_marker`).
    pub stereotype: Option<String>,
    /// Custom marker `(letter, color)` from `<<(L, #color) text>>`. The
    /// letter overrides the kind-default chip glyph; the color is a
    /// raw color spec (`#ABCDEF`, `red`, …) or `None` for the kind
    /// default.
    pub stereotype_marker: Option<StereotypeMarker>,
    /// Raw color spec (`#LightBlue`, `#ABC`). Codegen normalizes.
    pub fill: Option<String>,
    pub line: usize,
    /// Shape-specific extras. Pattern-matched at codegen / painter
    /// boundary; cheap to add a variant later.
    pub kind_data: EntityKindData,
}

/// Custom stereotype marker — overrides the default kind-letter chip.
/// Built from `<<(L, #color) text>>` syntax.
#[derive(Clone, Debug)]
pub struct StereotypeMarker {
    pub letter: String,
    pub color: Option<String>,
}

/// Shape-specific data carried by an [`Entity`]. The variant is
/// determined by the parser at declaration time and is invariant for
/// the entity's lifetime.
#[derive(Clone, Debug)]
pub enum EntityKindData {
    /// Class-family node (class / interface / abstract / enum /
    /// annotation / struct / exception / protocol / entity-shape).
    /// Painted as a compartment box; `kind` selects the marker glyph
    /// and stroke style.
    Compartment {
        kind: ClassFamilyKind,
        generic: Option<String>,
        fields: Vec<Member>,
        methods: Vec<Member>,
    },
    /// Free-text annotation rendered as a yellow dog-eared sticky note.
    Note { body: String },
    /// Object / instance entity — `name = value` rows, no method
    /// compartment, no marker chip. (Reserved for `object` syntax;
    /// not produced by the parser today.)
    Object { fields: Vec<ObjectField> },
    /// Desc-family shape with no compartment (actor, usecase,
    /// component, cloud, database, …). `members` holds optional
    /// inline `{ + foo }` content; usually empty.
    Plain { members: Vec<Member> },
}

impl EntityKindData {
    /// Convenience accessor — returns the field-row slice for entities
    /// that have one, empty otherwise.
    pub fn fields(&self) -> &[Member] {
        match self {
            EntityKindData::Compartment { fields, .. } => fields,
            EntityKindData::Plain { members } => members,
            _ => &[],
        }
    }

    /// Convenience accessor — returns the method-row slice for entities
    /// that have one, empty otherwise.
    pub fn methods(&self) -> &[Member] {
        match self {
            EntityKindData::Compartment { methods, .. } => methods,
            _ => &[],
        }
    }

    /// Generic parameter list (`<T, U>`) — only meaningful for
    /// compartment entities.
    pub fn generic(&self) -> Option<&str> {
        match self {
            EntityKindData::Compartment { generic, .. } => generic.as_deref(),
            _ => None,
        }
    }

    /// Note body — only meaningful for [`EntityKindData::Note`].
    pub fn note_body(&self) -> Option<&str> {
        match self {
            EntityKindData::Note { body } => Some(body.as_str()),
            _ => None,
        }
    }
}

/// Class-family sub-shape. Used by [`EntityKindData::Compartment`] to
/// pick the marker glyph and the painter's stroke/style variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClassFamilyKind {
    Class,
    Interface,
    Abstract,
    Enum,
    Annotation,
    Struct,
    Exception,
    Protocol,
    /// `entity` keyword in class-flavor (ER-style entity, not the
    /// description-family entity shape).
    EntityShape,
}

impl ClassFamilyKind {
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
        }
    }

    /// Single-letter glyph for the stereotype circle (`C` / `I` / `A` / `E`).
    /// `None` for shapes that don't get a marker.
    pub fn marker_letter(self) -> Option<char> {
        Some(match self {
            Self::Class | Self::Struct | Self::Exception => 'C',
            Self::Interface | Self::Protocol => 'I',
            Self::Abstract => 'A',
            Self::Enum => 'E',
            Self::Annotation => '@',
            Self::EntityShape => 'E',
        })
    }
}

/// Object-field row (`name = value`). Reserved for PlantUML object
/// diagrams; not produced by the parser today.
#[derive(Clone, Debug)]
pub struct ObjectField {
    pub name: String,
    pub value: String,
    pub line: usize,
}

/// Visual shape registry, aligned to PlantUML's `USymbols.java`. Entity
/// and Container share this enum — `database Foo` (leaf) and `database
/// "Cluster" { ... }` (container) both paint the same cylinder shape,
/// just with different content slots.
///
/// Variants beyond the ones the parser currently emits are placeholders
/// for the M5–M8 painter rollout; codegen treats them as fallbacks to
/// `Rectangle` until their painter lands.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum USymbol {
    /// "No symbol" — class-family compartment box (`class Foo { … }`).
    /// Painter dispatches to `_layout-class`.
    None,

    // Component family.
    Component,
    ComponentUml1,
    ComponentRectangle,
    /// Lollipop interface (`() Foo` / `interface` in desc-flavor).
    Interface,
    Port,
    PortIn,
    PortOut,

    // Deployment family.
    Node,
    Database,
    Cloud,
    Queue,
    Stack,
    Storage,
    Artifact,

    // Use-case family.
    Actor,
    ActorBusiness,
    ActorAwesome,
    ActorHollow,
    UseCase,
    UseCaseBusiness,

    // Universal containers / leaves.
    Package,
    Rectangle,
    Card,
    Folder,
    Frame,
    File,
    Hexagon,
    Agent,
    Person,
    Collections,

    // Activity-style shared with desc family.
    Action,
    Process,
    Label,
    Boundary,
    Control,
    EntityDomain,

    // Specials.
    Note,
    Diamond,
}

impl USymbol {
    /// Keyword written into the painter's `usymbol:` slot. M5+ adds
    /// per-symbol painters; v1 painter only recognizes `none` / `note` /
    /// `lollipop` / `package` / `namespace` / `folder` / `frame` /
    /// `cloud` / `node` / `together`.
    pub fn keyword(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Component => "component",
            Self::ComponentUml1 => "component-uml1",
            Self::ComponentRectangle => "component-rect",
            Self::Interface => "interface",
            Self::Port => "port",
            Self::PortIn => "port-in",
            Self::PortOut => "port-out",
            Self::Node => "node",
            Self::Database => "database",
            Self::Cloud => "cloud",
            Self::Queue => "queue",
            Self::Stack => "stack",
            Self::Storage => "storage",
            Self::Artifact => "artifact",
            Self::Actor => "actor",
            Self::ActorBusiness => "actor-business",
            Self::ActorAwesome => "actor-awesome",
            Self::ActorHollow => "actor-hollow",
            Self::UseCase => "usecase",
            Self::UseCaseBusiness => "usecase-business",
            Self::Package => "package",
            Self::Rectangle => "rectangle",
            Self::Card => "card",
            Self::Folder => "folder",
            Self::Frame => "frame",
            Self::File => "file",
            Self::Hexagon => "hexagon",
            Self::Agent => "agent",
            Self::Person => "person",
            Self::Collections => "collections",
            Self::Action => "action",
            Self::Process => "process",
            Self::Label => "label",
            Self::Boundary => "boundary",
            Self::Control => "control",
            Self::EntityDomain => "entity-domain",
            Self::Note => "note",
            Self::Diamond => "diamond",
        }
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
    /// `(` — provides-interface socket. Reserved for M7.
    SocketOpen,
    /// `)` — consumes-interface socket. Reserved for M7.
    SocketClosed,
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

/// Container for `package { }` / `namespace { }` / `together { }` /
/// `folder { }` / `frame { }` / `node { }` / `cloud { }` blocks (and
/// the M5+ extension: any of the 17 desc-family container keywords).
/// `usymbol` selects the painter (folder-tab vs. cloud vs. cylinder
/// etc.); `together` is the special "anonymous virtual group" case
/// (PlantUML's `together { … }`) — it never paints a border / label.
#[derive(Clone, Debug)]
pub struct Container {
    /// Shape; shares the [`USymbol`] registry with [`Entity`]. v1
    /// painter only knows the original 7 container shapes (Package,
    /// Folder, Frame, Cloud, Node, plus None for together); new
    /// variants land in M5+.
    pub usymbol: USymbol,
    /// `true` when this container is a PlantUML `together { … }` group
    /// — anonymous and borderless. The painter skips drawing the frame
    /// and label band; layout still uses the cluster bbox to keep the
    /// `together` children visually grouped.
    pub together: bool,
    pub label: String,
    pub stereotype: Option<String>,
    pub children_entities: Vec<String>,
    pub children_containers: Vec<usize>,
    pub line: usize,
}

impl Container {
    /// Whether this container draws a label band that needs measuring.
    /// `together` is anonymous; everything else with a non-empty label
    /// gets a band.
    pub fn has_label_band(&self) -> bool {
        !self.together && !self.label.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Activity diagram (PlantUML `activitydiagram3` / new syntax).
//
// IR is a **statement tree** — order is encoded in tree position, no edges.
// See `docs/activity-diagram-design.md` for the full rationale and the
// painter contract.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// State diagram (PlantUML `@startuml` with `[*]` / `state` declarations).
//
// IR is a flat node table + transition list + a side table for the nesting
// (`StateNode.parent` / `.children`) and concurrent regions (`RegionGroup`).
// See `docs/state-diagram-design.md` for the design rationale and the
// PlantUML-side references.
// ---------------------------------------------------------------------------

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
    /// `note "..." as Nx` — a standalone note.
    Floating { id: String },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BorderStyle {
    Solid,
    Dashed,
    Bold,
    Dotted,
}
