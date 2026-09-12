// Flowchart silhouettes. Geometry is shared by probes and the final painter.
// Fixed contour anchors match Rust codegen/flowchart/geom; dimensions may be locked
// by the second pass or by the explicit no-measure path.
#let _layout-flow(spec, fill, stroke) = {
  let kind = spec.kind
  let label = spec.at("name", default: [])
  let m = measure(label)
  let w = m.width + 24pt
  let h = calc.max(28pt, m.height + 16pt)
  if kind == "flow-diamond" { w *= 2; h *= 2 }
  if kind == "flow-circle" {
    let d = calc.sqrt(w.pt() * w.pt() + h.pt() * h.pt()) * 1pt
    w = d; h = d
  }
  if kind == "flow-stadium" { w += h }
  if kind == "flow-cylinder" { h += 12pt }
  if kind == "flow-asymmetric" { w += 24pt }
  w = spec.at("width", default: w)
  h = spec.at("height", default: h)
  let content = block(width: w, height: h, breakable: false, {
    let shape = if kind == "flow-diamond" {
      polygon((w/2, 0pt), (w, h/2), (w/2, h), (0pt, h/2), fill: fill, stroke: stroke)
    } else if kind == "flow-circle" {
      ellipse(width: w, height: h, fill: fill, stroke: stroke)
    } else if kind == "flow-asymmetric" {
      polygon((0pt, 0pt), (w, 0pt), (w, h), (0pt, h), (12pt, h/2), fill: fill, stroke: stroke)
    } else if kind == "flow-cylinder" {
      block(width: w, height: h, {
        place(top + left, dy: 6pt, rect(width: w, height: h - 12pt, fill: fill, stroke: stroke))
        place(top + left, dy: h - 12pt, ellipse(width: w, height: 12pt, fill: fill, stroke: stroke))
        // Cover the body's bottom seam, keeping the curved outer cap.
        place(top + left, dx: 1pt, dy: h - 13pt, rect(width: w - 2pt, height: 7pt, fill: fill, stroke: none))
        place(top + left, ellipse(width: w, height: 12pt, fill: fill, stroke: stroke))
      })
    } else {
      let r = if kind == "flow-stadium" { h/2 } else if kind == "flow-rounded" { 6pt } else { 0pt }
      rect(width: w, height: h, radius: r, fill: fill, stroke: stroke)
    }
    place(top + left, shape)
    place(top + left, dx: (w - m.width)/2, dy: (h - m.height)/2, label)
  })
  (
    content: content, width: w, height: h, mid-x: w/2, mid-y: h/2,
    anchor-top: (w/2, 0pt), anchor-bot: (w/2, h),
    anchor-left: (if kind == "flow-asymmetric" { 12pt } else { 0pt }, h/2),
    anchor-right: (w, h/2),
  )
}
