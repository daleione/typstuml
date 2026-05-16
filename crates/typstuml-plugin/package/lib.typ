// typstuml — render PlantUML diagrams inside a Typst document.
//
// Public API:
//   #render-puml(source: str)         -> content
//
// Loading a `.puml` file from disk is the caller's job:
//   #render-puml(read("diagrams/login.puml"))
//
// Typst sandboxes `read()` to the project root of the .typ file that
// calls it, so the read MUST happen in user code; a `render-puml-file`
// helper inside this library would (per Typst's package model) look
// in the package install directory, not the user's document.
//
// Architecture:
//   The PlantUML parsing, layout, and code-generation all happen in the
//   `typstuml.wasm` plugin (a stripped Rust build of the typstuml crate
//   minus its native Typst-as-library renderer). The plugin returns
//   Typst source that we `eval()` against a scope that maps every
//   blockcell symbol the codegen emits.
//
//   For measurement-aware diagrams (class / record-graph / state) the
//   layout needs real text widths. We do a two-pass measurement
//   round-trip on the Typst side: hide-eval the pass-1 probe source,
//   `query(<typstuml_measure>)` the resulting metadata, send those
//   measurements back through the plugin, then eval the pass-2 source.
//   For diagrams with no probes (sequence / activity / mindmap / wbs)
//   we fast-path straight to pass-2.

#import "blockcell/lib.typ" as bc

#let _plugin = plugin("typstuml.wasm")

// ---------------------------------------------------------------------------
// Protocol version guard. Bumped together with PROTOCOL_VERSION in the
// typstuml-plugin Rust crate on any wire-format break. lib.typ and
// typstuml.wasm must ship together — a mismatch usually means the user
// has a stale @local install.
// ---------------------------------------------------------------------------
#assert.eq(
  int.from-bytes(_plugin.protocol_version(), endian: "little"),
  1,
  message: "typstuml: lib.typ / typstuml.wasm version mismatch; reinstall the package",
)

// ---------------------------------------------------------------------------
// Eval scope: every blockcell symbol the plugin's codegen emits at the
// Typst side. Mirror of `crate::codegen::REFERENCED_BLOCKCELL_SYMBOLS`
// and `build.rs::STAGED_LIB_TYP` — three-way drift goes through CI when
// goldens diverge.
// ---------------------------------------------------------------------------
#let _bc-scope = (
  // records (graph painter + pass-1 probe)
  "record-layout":    bc.record-layout,
  "record-probe":     bc.record-probe,

  // sequence
  "seq-puml":         bc.seq-puml,

  // tree / wbs / mindmap
  "tree":             bc.tree,
  "node":             bc.node,
  "mindmap":          bc.mindmap,

  // cuca (class / use-case / component painters + probes)
  "cuca-layout":      bc.cuca-layout,
  "cuca-probe":       bc.cuca-probe,
  "container-probe":  bc.container-probe,

  // state diagrams
  "state-layout":     bc.state-layout,
  "state-probe":      bc.state-probe,
  "state-note-probe": bc.state-note-probe,

  // activity atoms passed as args inside activity painters
  "process":          bc.process,
  "decision":         bc.decision,
  "terminal":         bc.terminal,
  "junction":         bc.junction,
  "edge":             bc.edge,
  "flow-node":        bc.flow-node,

  // activity composites / containers
  "flow-col":         bc.flow-col,
  "section":          bc.section,

  // activity flow constructors / decorators
  "branch":           bc.branch,
  "branch-merge":     bc.branch-merge,
  "switch":           bc.switch,
  "case":             bc.case,
  "n-way":            bc.n-way,
  "fork-bar":         bc.fork-bar,
  "flow-loop":        bc.flow-loop,
  "start-marker":     bc.start-marker,
  "stop-marker":      bc.stop-marker,
  "end-marker":       bc.end-marker,
  "detach-marker":    bc.detach-marker,
  "partition":        bc.partition,
  "flow-note":        bc.flow-note,
  "with-notes":       bc.with-notes,
  "swimlane":         bc.swimlane,
  "lane":             bc.lane,
)

// Translate the metadata dicts written by blockcell's `*-probe` painters
// (`{id, w, h, row_centers?}`) into the wire shape the plugin's
// `emit_layout` expects: an array of `{id, width_pt, height_pt, row_centers}`
// encoded as CBOR. Field renames match `ProbeEntry` in
// `crates/typstuml-plugin/src/lib.rs`.
#let _encode-measurements(values) = {
  let normalized = values.map(v => (
    id:          v.id,
    width_pt:    v.w,
    height_pt:   v.h,
    row_centers: v.at("row_centers", default: ()),
  ))
  cbor.encode(normalized)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a PlantUML source string and return its content. Returned as
/// contextual content (so multiple-diagram inputs can interleave with
/// the host document's flow). Throws a Typst error with the plugin's
/// message on parse / codegen failure.
#let render-puml(source) = {
  let src-bytes = bytes(source)
  let probe-src-bytes = _plugin.emit_probes(src-bytes)

  if probe-src-bytes.len() == 0 {
    // No measurement-aware diagram in this source — skip the round-trip.
    let layout-src = str(_plugin.emit_layout_no_measure(src-bytes))
    eval(layout-src, mode: "markup", scope: _bc-scope)
  } else {
    // Measurement round-trip. The `context` block waits for introspection
    // to converge before `query()` returns probe metadata.
    context {
      hide(eval(str(probe-src-bytes), mode: "markup", scope: _bc-scope))
      let probes = query(<typstuml_measure>).map(it => it.value)
      let layout-src = str(_plugin.emit_layout(src-bytes, _encode-measurements(probes)))
      eval(layout-src, mode: "markup", scope: _bc-scope)
    }
  }
}

