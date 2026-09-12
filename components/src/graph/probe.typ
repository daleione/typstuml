// Measure the *label content* of a `package` / `namespace` / similar
// container as `cuca-layout`'s package painter would render it: bold
// 0.85em text, no insets included. Callers add the painter's left /
// right / top / bottom margins themselves to derive the outer band
// dimensions.
//
// Returns w / h as float pt via the `<typstuml_measure>` metadata
// channel.
#let container-probe(measure-scope: none,
  id: none,
  label: [],
) = context {
  let m = measure(text(weight: "bold", size: 0.85em, label))
  [#metadata((id: id, w: m.width.pt(), h: m.height.pt()) + if measure-scope == none { (:) } else { (scope: measure-scope,) }) <typstuml_measure>]
}

// Measure an edge label exactly as `_place-edge-label` renders it (the
// 2pt-inset box around 0.78em text; fill doesn't affect measurement).
// The ELK-engine layout feeds this size into the layered pipeline so
// the label gets its own reserved space (LABEL dummy) instead of being
// placed post-hoc on the routed polyline.
#let graph-edge-label-probe(measure-scope: none,
  id: none,
  label: [],
) = context {
  let m = measure(box(inset: 2pt, text(size: 0.78em, label)))
  [#metadata((id: id, w: m.width.pt(), h: m.height.pt()) + if measure-scope == none { (:) } else { (scope: measure-scope,) }) <typstuml_measure>]
}
