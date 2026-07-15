// ============================================================================
// Class-card theme: stereotype tint table + compartment row renderer.
// ============================================================================
//
// Shared by `_layout-class` (the 3-compartment card) for its corner chip
// and by `_render-member` for visibility-glyph styling. The desc-family
// shape painters in `shape-desc.typ` do not consult this — they paint a
// uniform fill and need no stereotype glyph.

#import "../palettes.typ": palettes

// Default tint and single-letter glyph for the stereotype circle, keyed by
// entity kind. Loosely mirrors PlantUML's default skin
// (orange/blue/lavender/pink). Codegen passes a `kind` string we look up
// here; unknown kinds fall back to gray + no glyph so a user-defined
// stereotype still renders something readable.
#let _kind-styles = (
  "class":      (fill: palettes.pastel.green, letter: "C"),
  "struct":     (fill: palettes.pastel.green, letter: "C"),
  "exception":  (fill: palettes.pastel.green, letter: "C"),
  "interface":  (fill: palettes.pastel.purple, letter: "I"),
  "protocol":   (fill: palettes.pastel.purple, letter: "I"),
  "abstract":   (fill: palettes.pastel.teal, letter: "A"),
  "enum":       (fill: palettes.pastel.orange, letter: "E"),
  "annotation": (fill: palettes.pastel.red, letter: "@"),
  "entity":     (fill: palettes.pastel.green, letter: "E"),
)

#let _kind-style(kind) = _kind-styles.at(kind,
  default: (fill: palettes.pastel.gray, letter: none))

// Render one compartment row (a field or method). `member` is a dict with
// keys `vis` (string ∈ {"+", "-", "#", "~", ""}), `body` (content),
// `static` (bool), `abstract` (bool). The visibility glyph is rendered
// monospace so a column of `+`/`-`/`#`/`~` lines up cleanly.
#let _render-member(member) = {
  let vis = member.at("vis", default: "")
  let body = member.at("body", default: [])
  let is-static = member.at("static", default: false)
  let is-abstract = member.at("abstract", default: false)
  let glyph = if vis == "" { [] } else {
    text(font: ("DejaVu Sans Mono", "Menlo", "Consolas"),
         size: 0.95em, fill: palettes.base.text-muted, vis)
  }
  let rendered = body
  if is-abstract { rendered = emph(rendered) }
  if is-static { rendered = underline(rendered) }
  // Visibility glyph + thin gap + body. Inline so a row's height is one
  // text line.
  if vis == "" { rendered }
  else {
    glyph
    h(0.35em)
    rendered
  }
}
