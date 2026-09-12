// Shared positioned-node canvas. Painters supply measured shape geometry;
// this module draws containers, routed edges and labels without language dispatch.
#import "../palettes.typ": palettes
#import "edges.typ": _draw-edge, _place-edge-label

#let graph-layout(
  paint: none,
  include-negative-path: false,
  title: none,
  nodes: (),
  edges: (),
  containers: (),
  direction: "tb",
  bg-color: white,
  default-fill: palettes.pastel.blue,
  stroke: 1pt + palettes.base.border-soft,
  inner-stroke: 1pt + palettes.base.border-soft,
  radius: 6pt,
  inset: (x: 0.6em, y: 0.3em),
  edge-color: palettes.base.border-soft,
  edge-thickness: 1.5pt,
  head-size: 6pt,
  package-stroke: 1pt + palettes.base.border-soft,
  package-fill: none,
) = context {
  let head-size = head-size.to-absolute()

  let metas = nodes.map(spec =>
    paint(spec, default-fill, stroke, inner-stroke, radius, inset))

  let is-lr = direction == "lr"
  // Edge anchors snapped to the painter's measured geometry. Each meta
  // may declare `anchor-{top,bot,left,right}` points relative to its
  // local frame (lollipops use this so edges attach to the disc,
  // not below the label that hangs off the layout box). When absent
  // we fall back to the box midpoints.
  let local-anchor(i, side) = {
    let m = metas.at(i)
    let key = "anchor-" + side
    if key in m {
      m.at(key)
    } else if side == "top" {
      (m.mid-x, 0pt)
    } else if side == "bot" {
      (m.mid-x, m.height)
    } else if side == "left" {
      (0pt, m.mid-y)
    } else { // "right"
      (m.width, m.mid-y)
    }
  }
  let world-anchor(i, side) = {
    let local = local-anchor(i, side)
    (nodes.at(i).x + local.at(0), nodes.at(i).y + local.at(1))
  }
  // Per-edge `from-side` / `to-side` overrides take precedence (so
  // sibling-cluster edges that go side-to-side don't get forced into
  // bot/top anchoring). The defaults follow `direction`. An optional
  // `from-x` / `from-y` / `to-x` / `to-y` overrides the anchor's
  // free-axis coordinate (y for left/right sides, x for top/bot)
  // — codegen sets this when it wants to align both anchors so the
  // Manhattan route collapses to a single segment.
  let default-from-side = if is-lr { "right" } else { "bot" }
  let default-to-side = if is-lr { "left" } else { "top" }
  let resolved-anchor(i, side, override-x, override-y) = {
    let p = world-anchor(i, side)
    let px = p.at(0)
    let py = p.at(1)
    if (side == "top" or side == "bot") and override-x != none {
      px = override-x
    }
    if (side == "left" or side == "right") and override-y != none {
      py = override-y
    }
    // For ellipse-shaped entities (usecase) the bbox corners sit
    // outside the visible silhouette, so an off-midpoint anchor lands
    // in a visible gap between the line end and the curved boundary.
    // Project the anchor onto the actual ellipse where it crosses the
    // override coord so the arrowhead sits flush with the shape.
    if metas.at(i).at("boundary", default: "box") == "ellipse" {
      let m = metas.at(i)
      let cx = nodes.at(i).x + m.mid-x
      let cy = nodes.at(i).y + m.mid-y
      let a = m.width / 2
      let b = m.height / 2
      if (side == "left" or side == "right") and override-y != none {
        let dy = py - cy
        let frac = 1 - (dy / b) * (dy / b)
        if frac > 0 {
          let dx = a * calc.sqrt(frac)
          px = if side == "left" { cx - dx } else { cx + dx }
        }
      }
      if (side == "top" or side == "bot") and override-x != none {
        let dx = px - cx
        let frac = 1 - (dx / a) * (dx / a)
        if frac > 0 {
          let dy = b * calc.sqrt(frac)
          py = if side == "top" { cy - dy } else { cy + dy }
        }
      }
    }
    (px, py)
  }
  let from-anchor(i, override-side) = resolved-anchor(
    i,
    if override-side != none { override-side } else { default-from-side },
    none, none,
  )
  let to-anchor(i, override-side) = resolved-anchor(
    i,
    if override-side != none { override-side } else { default-to-side },
    none, none,
  )

  // Canvas size = farthest extent across nodes, containers, and bezier
  // handles. Packages can extend further than their members because of
  // their padding band; include them explicitly. Classes (and edge
  // handles) can also dip into negative x/y when codegen has pushed
  // an association class past the chord — the shift below compensates.
  let canvas-x0 = 0pt
  let canvas-y0 = 0pt
  let canvas-w = 0pt
  let canvas-h = 0pt
  for i in range(nodes.len()) {
    let r = nodes.at(i)
    let m = metas.at(i)
    canvas-w = calc.max(canvas-w, r.x + m.width)
    canvas-h = calc.max(canvas-h, r.y + m.height)
    canvas-x0 = calc.min(canvas-x0, r.x)
    canvas-y0 = calc.min(canvas-y0, r.y)
  }
  for p in containers {
    canvas-w = calc.max(canvas-w, p.x + p.w)
    canvas-h = calc.max(canvas-h, p.y + p.h)
    canvas-x0 = calc.min(canvas-x0, p.x)
    canvas-y0 = calc.min(canvas-y0, p.y)
  }
  for e in edges {
    for seg in e.path {
      for p in (seg.c1, seg.c2, seg.end) {
        canvas-w = calc.max(canvas-w, p.at(0))
        canvas-h = calc.max(canvas-h, p.at(1))
        if include-negative-path {
          canvas-x0 = calc.min(canvas-x0, p.at(0))
          canvas-y0 = calc.min(canvas-y0, p.at(1))
        }
      }
    }
    // Engine-placed edge labels (absolute `label-pos` centers) can sit
    // beside the outermost trunk, past every node/package box — the ELK
    // layout reserves that space, so the canvas must include it. The box
    // mirrors `_place-edge-label`'s construction (fill doesn't measure).
    let lbl = e.at("label", default: none)
    let lpos = e.at("label-pos", default: none)
    if lbl != none and lpos != none {
      let m = measure(box(inset: 2pt, text(size: 0.78em, lbl)))
      canvas-w = calc.max(canvas-w, lpos.at(0) + m.width / 2)
      canvas-h = calc.max(canvas-h, lpos.at(1) + m.height / 2)
      canvas-x0 = calc.min(canvas-x0, lpos.at(0) - m.width / 2)
      canvas-y0 = calc.min(canvas-y0, lpos.at(1) - m.height / 2)
    }
  }

  // If any package extends to negative coords (its outer pad pushes left
  // / above the layout origin), shift everything right / down so the
  // resulting block doesn't clip.
  let shift-x = if canvas-x0 < 0pt { -canvas-x0 } else { 0pt }
  let shift-y = if canvas-y0 < 0pt { -canvas-y0 } else { 0pt }
  let final-w = canvas-w + shift-x
  let final-h = canvas-h + shift-y

  // Nesting depth of package `i` — how many other containers fully
  // enclose it. Used to give inner containers a slightly richer (but
  // still very light) blue tint than their parents, so nesting reads
  // visually without resorting to a different hue per sibling package.
  let pkg-depth(i) = {
    let a = containers.at(i)
    let count = 0
    for (j, b) in containers.enumerate() {
      if j != i {
        // `b` is a strict ancestor of `a` when it encloses `a`'s bbox
        // on all sides and is strictly bigger in at least one
        // dimension (container padding guarantees a true parent
        // always is; this also excludes `a` matching itself).
        let encloses = (
          b.x <= a.x and b.y <= a.y
          and (b.x + b.w) >= (a.x + a.w) and (b.y + b.h) >= (a.y + a.h)
          and (b.w > a.w or b.h > a.h)
        )
        if encloses { count = count + 1 }
      }
    }
    count
  }
  // Deliberately much lighter than any component fill (the lightest
  // component tint is the raw `palettes.pastel.*` swatch) — the
  // package needs to read as "pale backdrop", not compete with the
  // components sitting on top of it.
  let pkg-tint(depth) = {
    let tints = (
      palettes.pastel.blue.lighten(88%),
      palettes.pastel.blue.lighten(80%),
      palettes.pastel.blue.lighten(72%),
    )
    tints.at(calc.min(depth, tints.len() - 1))
  }

  let body = block(width: final-w, height: final-h, breakable: false, {
    // Packages first, so nodes draw on top of the labeled rectangles.
    // `together` is anonymous and rendered with no fill / dashed border
    // to visually mark it as a soft hint rather than a real container.
    for (i, p) in containers.enumerate() {
      let kind = p.at("kind", default: "package")
      let label = p.at("label", default: [])
      let stereotype = p.at("stereotype", default: none)
      let is-together = kind == "together"
      let pkg-fill = if is-together { none }
        else if package-fill != none { package-fill }
        else { pkg-tint(pkg-depth(i)) }
      let pkg-stroke = if is-together {
        (paint: palettes.base.text-muted, thickness: 0.5pt, dash: "dashed")
      } else { package-stroke }
      place(top + left, dx: p.x + shift-x, dy: p.y + shift-y,
        rect(width: p.w, height: p.h, fill: pkg-fill, stroke: pkg-stroke,
          radius: 8pt))
      if not is-together and label != [] {
        // Header strip at the top of the rectangle (~14pt). Label is
        // bold, slight inset from the left.
        place(top + left, dx: p.x + shift-x + 6pt, dy: p.y + shift-y + 2pt,
          text(weight: "bold", size: 0.85em, label))
      }
      if stereotype != none {
        place(top + right, dx: -(final-w - (p.x + shift-x + p.w)) - 6pt,
              dy: p.y + shift-y + 2pt,
          text(size: 0.7em, fill: palettes.base.text-muted, [«#stereotype»]))
      }
    }
    // Classes.
    for i in range(nodes.len()) {
      let r = nodes.at(i)
      place(top + left, dx: r.x + shift-x, dy: r.y + shift-y, metas.at(i).content)
    }
    // Edges. Source = bottom-mid of `from`; target = top-mid of `to`.
    // Codegen ensures Sugiyama TB ordering so this anchoring is sane;
    // see codegen/cuca::orient_relation for the swap rule.
    let shift-pt(p) = (p.at(0) + shift-x, p.at(1) + shift-y)
    for e in edges {
      // Couple-link edges (`(A, B) -- C`) carry an explicit `start` and
      // a `from-couple: (a, b)` index pair instead of `from`. We honor
      // the explicit start; the regular `from` lookup is skipped.
      let raw-start = if e.at("from", default: none) == none {
        e.at("start")
      } else {
        resolved-anchor(
          e.from,
          if "from-side" in e { e.from-side } else { default-from-side },
          e.at("from-x", default: none),
          e.at("from-y", default: none),
        )
      }
      let raw-end = resolved-anchor(
        e.to,
        if "to-side" in e { e.to-side } else { default-to-side },
        e.at("to-x", default: none),
        e.at("to-y", default: none),
      )
      let start = shift-pt(raw-start)
      let end = shift-pt(raw-end)
      // Path segments need the same shift since they're absolute coords.
      let shifted-path = e.path.map(seg => (
        c1: shift-pt(seg.c1),
        c2: shift-pt(seg.c2),
        end: shift-pt(seg.end),
      ))
      let style = e.at("style", default: "solid")
      let color = e.at("color", default: edge-color)
      let from-overridden = (e.at("from-x", default: none) != none) or (e.at("from-y", default: none) != none)
      let to-overridden = (e.at("to-x", default: none) != none) or (e.at("to-y", default: none) != none)
      _draw-edge(
        start, shifted-path, end,
        e.at("head-from", default: "none"),
        e.at("head-to", default: "none"),
        style, color, bg-color, edge-thickness * e.at("weight", default: 1), head-size * calc.sqrt(e.at("weight", default: 1)),
        from-side: if "from-side" in e { e.from-side } else { default-from-side },
        to-side: if "to-side" in e { e.to-side } else { default-to-side },
        // When codegen explicitly placed the anchor (e.g. sibling
        // distribution along a shared face), trust its emitted tangent
        // and skip the axis collapse — otherwise the head would be
        // forced perpendicular to the face even when the line arrives
        // at a diagonal.
        from-axis-snap: not from-overridden,
        to-axis-snap: not to-overridden,
      )
      // Couple-link "apoint" — small filled dot on the A-B chord
      // marking where the association class is attached. PlantUML
      // renders this as a 2pt black disc.
      if e.at("from-couple", default: none) != none {
        let dot-r = 1.5pt
        place(top + left,
          dx: start.at(0) - dot-r, dy: start.at(1) - dot-r,
          circle(radius: dot-r, fill: color, stroke: none))
      }
      let label-pos = e.at("label-pos", default: none)
      _place-edge-label(start, end, 0.5, e.at("label", default: none),
        label-pos: if label-pos == none { none } else { shift-pt(label-pos) },
        nodes: nodes, metas: metas, shift-x: shift-x, shift-y: shift-y)
      // Mult and role share the same `t`; they're split apart by a small
      // perpendicular offset so both fit beside the edge without
      // overlapping. Positive perp = chord's left, negative = right.
      _place-edge-label(start, end, 0.12, e.at("mult-from", default: none),
        perp: 10pt,
        nodes: nodes, metas: metas, shift-x: shift-x, shift-y: shift-y)
      _place-edge-label(start, end, 0.12, e.at("role-from", default: none),
        perp: -10pt,
        nodes: nodes, metas: metas, shift-x: shift-x, shift-y: shift-y)
      _place-edge-label(start, end, 0.88, e.at("mult-to", default: none),
        perp: 10pt,
        nodes: nodes, metas: metas, shift-x: shift-x, shift-y: shift-y)
      _place-edge-label(start, end, 0.88, e.at("role-to", default: none),
        perp: -10pt,
        nodes: nodes, metas: metas, shift-x: shift-x, shift-y: shift-y)
      // `note on link`: a tiny yellow sticky next to the chord midpoint.
      let edge-note = e.at("note", default: none)
      if edge-note != none {
        let mx = start.at(0) + (end.at(0) - start.at(0)) * 0.5
        let my = start.at(1) + (end.at(1) - start.at(1)) * 0.5
        let nbody = text(size: 0.78em, edge-note)
        let nm = measure(nbody)
        let pad = 2pt
        let nw = nm.width + 2 * pad
        let nh = nm.height + 2 * pad
        // Offset perpendicular to the chord so the sticky doesn't sit
        // on top of the line.
        let dx = end.at(0) - start.at(0)
        let dy = end.at(1) - start.at(1)
        let len-pt = calc.sqrt((dx / 1pt) * (dx / 1pt) + (dy / 1pt) * (dy / 1pt))
        let (px, py) = if len-pt == 0 { (0, 0) }
          else { (-dy / (len-pt * 1pt), dx / (len-pt * 1pt)) }
        let off = 14pt
        place(top + left,
          dx: mx + px * off - nw / 2,
          dy: my + py * off - nh / 2,
          box(width: nw, height: nh,
              fill: palettes.pastel.yellow,
              stroke: 0.4pt + palettes.base.border-soft,
              place(center + horizon, nbody)))
      }
    }
  })

  if title != none {
    align(center)[#strong(title)]
    v(0.5em, weak: true)
  }
  body
}
