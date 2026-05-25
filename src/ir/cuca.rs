//! Cuca diagram IR — PlantUML's class / component / deployment / use-case /
//! object families in one IR. See `docs/cuca-diagram-design.md`.

use super::common::{Direction, LayoutDirection, LineStyle, Skinparam};

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
