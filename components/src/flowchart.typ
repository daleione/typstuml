// Flowchart painters and measurement. The canvas is shared with CUCA.
#import "graph/canvas.typ": graph-layout
#import "flowchart/shapes.typ": _layout-flow

#let _paint-flow(spec, default-fill, stroke, inner-stroke, radius, inset) = {
  _layout-flow(spec, spec.at("fill", default: default-fill), stroke)
}

#let flowchart-layout(nodes: (), subgraphs: (), ..args) = graph-layout(
  nodes: nodes, containers: subgraphs, paint: _paint-flow,
  include-negative-path: true, ..args,
)

#let flowchart-probe(measure-scope: none, id: none, spec: (:)) = context {
  let m = _layout-flow(spec, white, 1pt + black)
  [#metadata((id: id, w: m.width.pt(), h: m.height.pt()) + if measure-scope == none { (:) } else { (scope: measure-scope,) }) <typstuml_measure>]
}
