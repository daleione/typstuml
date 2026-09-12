// ============================================================================
// Cuca (description-family) diagrams: class / object / component /
// deployment / use case. Compartment / shape entities linked by arrows
// whose head shape encodes UML semantics (extends / aggregation /
// composition / association / dependency / interface socket).
// ============================================================================
//
// cuca-layout  Painter for cuca diagrams whose entity positions and edge
//              bezier paths are computed externally (TypstUML's
//              codegen/cuca). The painter is a pure layout consumer: it
//              does not run any graph algorithm. Codegen estimates per-
//              entity bboxes, runs Sugiyama (top-to-bottom rank
//              progression) and pathplan, and emits absolute positions
//              plus per-edge cubic-bezier segments.
//
// Implementation is split across `cuca/`:
//   theme.typ        kind→stereotype tint table + compartment row renderer
//   shape-card.typ   _layout-class / _layout-note / _layout-lollipop
//                    (the 3-compartment card family — the "real" class
//                    diagram core)
//   shape-desc.typ   the 27 desc-family shape painters (actor /
//                    component / database / node / usecase / cloud /
//                    rectangle / folder / frame / file / queue /
//                    storage / hexagon / card / artifact / collections /
//                    action / process / label / stack / agent / person /
//                    boundary / control / entity-domain)
//   edges.typ        edge head shapes, multi-segment bezier draw,
//                    label placement with class-bbox avoidance
// ============================================================================

#import "palettes.typ": palettes
#import "cuca/shape-card.typ": _layout-class, _layout-note, _layout-lollipop, _layout-object
#import "cuca/shape-desc.typ": *
#import "graph/canvas.typ": graph-layout
#import "graph/probe.typ": container-probe, graph-edge-label-probe

// Resolve a kind string to a painter result. Shared by `cuca-layout`
// (which passes the spec's `fill` and the global stroke / radius / inset
// for class cards) and `cuca-probe` (which passes neutral theme args
// since measurement is paint-independent).
#let _paint(spec, default-fill, class-stroke, inner-stroke, radius, inset) = {
  let kind = spec.at("kind", default: "class")
  let cls-fill = spec.at("fill", default: default-fill)
  if kind == "note" {
    _layout-note(spec, inset)
  } else if kind == "object" {
    _layout-object(spec, cls-fill, class-stroke, inner-stroke, radius, inset)
  } else if kind == "lollipop" or kind == "circle" {
    _layout-lollipop(spec)
  } else if kind == "actor" {
    _layout-actor(spec)
  } else if kind == "database" {
    _layout-database(spec, cls-fill)
  } else if kind == "component" {
    _layout-component(spec, cls-fill)
  } else if kind == "node" {
    _layout-node(spec, cls-fill)
  } else if kind == "usecase" {
    _layout-usecase(spec, cls-fill) + (boundary: "ellipse",)
  } else if kind == "cloud" {
    _layout-cloud(spec, cls-fill)
  } else if kind == "rectangle" {
    _layout-rectangle(spec, cls-fill)
  } else if kind == "folder" {
    _layout-folder(spec, cls-fill)
  } else if kind == "frame" {
    _layout-frame(spec, cls-fill)
  } else if kind == "file" {
    _layout-file(spec, cls-fill)
  } else if kind == "queue" {
    _layout-queue(spec, cls-fill)
  } else if kind == "storage" {
    _layout-storage(spec, cls-fill)
  } else if kind == "hexagon" {
    _layout-hexagon(spec, cls-fill)
  } else if kind == "card" {
    _layout-card(spec, cls-fill)
  } else if kind == "artifact" {
    _layout-artifact(spec, cls-fill)
  } else if kind == "collections" {
    _layout-collections(spec, cls-fill)
  } else if kind == "action" {
    _layout-action(spec, cls-fill)
  } else if kind == "process" {
    _layout-process(spec, cls-fill)
  } else if kind == "label" {
    _layout-label(spec)
  } else if kind == "stack" {
    _layout-stack(spec, cls-fill)
  } else if kind == "agent" {
    _layout-agent(spec, cls-fill)
  } else if kind == "person" {
    _layout-person(spec, cls-fill)
  } else if kind == "boundary" {
    _layout-boundary(spec, cls-fill)
  } else if kind == "control" {
    _layout-control(spec, cls-fill)
  } else if kind == "entity-domain" {
    _layout-entity-domain(spec, cls-fill)
  } else {
    _layout-class(spec, cls-fill, class-stroke, inner-stroke, radius, inset)
  }
}

/// Painter for cuca diagrams whose entity positions and edge bezier
/// paths are computed by codegen (TypstUML's `codegen/cuca`).
///
/// ```typst
/// #cuca-layout(
///   classes: (
///     (x: 0pt, y: 0pt, kind: "class", name: [Animal],
///      fields: ((vis: "+", body: [name: String]),),
///      methods: ((vis: "+", body: [speak()]),)),
///     (x: 0pt, y: 80pt, kind: "class", name: [Dog],
///      fields: (), methods: (())),
///   ),
///   edges: (
///     (from: 1, to: 0,
///      head-from: "none", head-to: "triangle-open",
///      style: "solid",
///      path: ((c1: (50pt, 70pt), c2: (50pt, 30pt), end: (50pt, 0pt)),)),
///   ),
/// )
/// ```
///
/// - `title`: optional bold title above the diagram.
/// - `classes`: array of dicts. Required keys: `x`, `y`, `kind`, `name`.
///   Optional: `generic`, `stereotype`, `fields`, `methods`, `fill`.
/// - `edges`: array of dicts. Required: `from`, `to`, `head-from`,
///   `head-to`, `style`, `path`. Optional: `label`, `mult-from`,
///   `mult-to`, `color`.
/// - `bg-color`: page background (used to fill "open" head shapes so the
///   underlying line doesn't show through). Defaults to white.
/// - `default-fill`: fallback fill when a class spec has no `fill`.
/// - `stroke` / `inner-stroke`: outer class border and compartment
///   separator strokes.
/// - `radius`: corner radius of class boxes.
/// - `inset`: per-cell padding inside class compartments as `(x:, y:)`.
/// - `edge-color` / `edge-thickness`: default edge stroke styling
///   (overridden per-edge by `color` in an edge dict).
/// - `head-size`: tip size for arrow / triangle / diamond / circle heads.
/// - `package-fill`: fill for package/frame container boxes. `none`
///   (the default) picks a soft blue tint that deepens slightly with
///   nesting depth (see `_pkg-tint`); pass an explicit color (e.g. from
///   `skinparam packageBackgroundColor`) to force one fill everywhere.
// Legacy public wire names are translated at the CUCA boundary.
#let cuca-layout(classes: (), packages: (), ..args) = graph-layout(
  nodes: classes, containers: packages, paint: _paint, ..args,
)

// ============================================================================
// Measure protocol
// ============================================================================
//
// `cuca-probe` measures the natural width / height of a single entity
// spec — the exact same value `cuca-layout` would compute for `total-w` /
// `total-h` when codegen does NOT pass `width:` / `height:` overrides.
// It emits a `metadata((id, w, h))` element with the `<typstuml_measure>`
// label; the TypstUML Rust runtime queries this label after a pass-1
// compile to read measurements back into the layout pipeline.
//
// Caller contract: `spec` MUST NOT carry `width:` / `height:` (those
// would short-circuit the natural-size computation). `x:` / `y:` are
// ignored if present.
//
// Defaults for `inset` MUST stay in sync with `cuca-layout`'s defaults
// above — pass-1 and pass-2 must use byte-identical inset for the
// measurement to be meaningful. Codegen should always pass `inset:` to
// both ends if it customizes the value.
#let cuca-probe(measure-scope: none,
  id: none,
  spec: (:),
  inset: (x: 0.6em, y: 0.3em),
) = context {
  // Use neutral theme args — they don't affect measurement, only paint.
  let m = _paint(spec, white, 1pt + black, 0.5pt + black, 4pt, inset)
  [#metadata((id: id, w: m.width.pt(), h: m.height.pt()) + if measure-scope == none { (:) } else { (scope: measure-scope,) }) <typstuml_measure>]
}

// Backward-compatible alias for the shared edge-label measurement primitive.
#let cuca-edge-label-probe = graph-edge-label-probe
