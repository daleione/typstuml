//! Activity-diagram codegen.
//!
//! Activity is a statement tree (see `docs/activity-diagram-design.md` §3):
//! there's no geometry, no edge routing, no measure pass — codegen is a
//! local recursive translation from [`ActivityStmt`] to a nested
//! `flow-col(...)` / `branch-merge(...)` / `n-way(...)` / `switch(...)` /
//! `flow-loop(...)` Typst expression. Painters do all the layout work.
//!
//! The output style intentionally mirrors the design-doc example so it
//! reads as the "canonical" Typst form for an activity diagram.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::ir::{
    ActionKind, ActivityDiagram, ActivityStmt, ElseIfBranch, NoteAttach, NotePosition,
    PartitionKind, Skinparam, SwitchCase,
};
use crate::layout::swimlane as sw_layout;
use crate::runtime::MeasurementSet;

pub fn emit(
    out: &mut String,
    act: &ActivityDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    emit_skinparam_preamble(out, &act.skinparams);
    if has_top_level_swimlane(&act.body) {
        match measurements {
            Some(set) => emit_swimlane_layout(out, act, set, diagram_idx),
            None => {
                // Measurement pass didn't run (or failed) — fall back to
                // the legacy grid painter so the diagram still renders,
                // even without cross-lane connectors.
                if let Some(t) = &act.title {
                    emit_title(out, t);
                }
                out.push_str("#align(center, swimlane(\n");
                emit_swimlane_lanes_legacy(out, &act.body, 1);
                out.push_str("))\n");
            }
        }
        return;
    }
    if let Some(t) = &act.title {
        emit_title(out, t);
    }
    out.push_str("#align(center, flow-col(\n");
    emit_stmts(out, &act.body, 1);
    out.push_str("))\n");
}

pub fn has_swimlane_probes(act: &ActivityDiagram) -> bool {
    has_top_level_swimlane(&act.body)
}

/// Emit one `swimlane-probe` per action / marker plus one per non-empty
/// lane label, so the measure pass returns natural sizes that the
/// solver in [`crate::layout::swimlane`] needs to pack lanes and place
/// nodes.
pub fn collect_probes(
    act: &ActivityDiagram,
    diagram_idx: usize,
    out: &mut String,
    expected_ids: &mut Vec<String>,
) {
    let (lanes, nodes) = linearize_swimlane(&act.body);
    for (i, lane) in lanes.iter().enumerate() {
        if lane.label.is_empty() {
            continue;
        }
        let id = format!("sw{diagram_idx}_lane{i}");
        let _ = writeln!(
            out,
            "#swimlane-probe(id: \"{}\", text(weight: \"bold\", size: 0.9em, [{}]))",
            id,
            typst_escape(&lane.label),
        );
        expected_ids.push(id);
    }
    for (i, n) in nodes.iter().enumerate() {
        let id = format!("sw{diagram_idx}_n{i}");
        out.push_str(&format!("#swimlane-probe(id: \"{id}\", "));
        out.push_str(&n.typst_expr);
        out.push_str(")\n");
        expected_ids.push(id);
    }
}

fn has_top_level_swimlane(body: &[ActivityStmt]) -> bool {
    body.iter()
        .any(|s| matches!(s, ActivityStmt::SwimlaneSwitch { .. }))
}

/// One linearized node in source order with its lane index and the
/// emitted Typst content for the node itself (a `process[...]`, a
/// marker, or — for intra-lane compounds — a full `branch-merge` /
/// `flow-loop` tree).
struct SwimlaneNode {
    lane: usize,
    typst_expr: String,
}

struct LaneSpec {
    label: String,
    color: Option<String>,
}

/// Walk `body` once, deduping lanes by label (so a lane revisit reuses
/// the same column) and emitting one [`SwimlaneNode`] per non-switch
/// statement. Compound statements that contain a `SwimlaneSwitch`
/// internally are still emitted as a single node — the inner switch is
/// silently dropped (matches the pre-existing M0 fallback behaviour);
/// proper nested handling is a follow-up.
fn linearize_swimlane(body: &[ActivityStmt]) -> (Vec<LaneSpec>, Vec<SwimlaneNode>) {
    let mut lanes: Vec<LaneSpec> = Vec::new();
    let mut lane_index: HashMap<String, usize> = HashMap::new();
    let mut nodes: Vec<SwimlaneNode> = Vec::new();
    let mut current: Option<usize> = None;

    for s in body {
        match s {
            ActivityStmt::SwimlaneSwitch { label, color, .. } => {
                let idx = if let Some(&i) = lane_index.get(label) {
                    i
                } else {
                    let i = lanes.len();
                    lanes.push(LaneSpec {
                        label: label.clone(),
                        color: color.clone(),
                    });
                    lane_index.insert(label.clone(), i);
                    i
                };
                current = Some(idx);
            }
            _ => {
                if current.is_none() {
                    // Implicit unnamed lane for any pre-switch content.
                    current = Some(lanes.len());
                    lanes.push(LaneSpec {
                        label: String::new(),
                        color: None,
                    });
                }
                let lane = current.unwrap();
                let mut expr = String::new();
                emit_stmt(&mut expr, s, 1);
                nodes.push(SwimlaneNode {
                    lane,
                    typst_expr: expr,
                });
            }
        }
    }
    (lanes, nodes)
}

fn emit_swimlane_layout(
    out: &mut String,
    act: &ActivityDiagram,
    measurements: &MeasurementSet,
    diagram_idx: usize,
) {
    let (lanes, nodes) = linearize_swimlane(&act.body);
    let lane_inputs: Vec<sw_layout::LaneInput> = lanes
        .iter()
        .map(|l| sw_layout::LaneInput {
            label: l.label.clone(),
            color: l.color.clone(),
        })
        .collect();
    let node_inputs: Vec<sw_layout::NodeInput> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| sw_layout::NodeInput {
            probe_id: format!("sw{diagram_idx}_n{i}"),
            lane: n.lane,
        })
        .collect();
    let lane_label_probe_ids: Vec<Option<String>> = lanes
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.label.is_empty() {
                None
            } else {
                Some(format!("sw{diagram_idx}_lane{i}"))
            }
        })
        .collect();

    let layout = sw_layout::solve(
        &lane_inputs,
        &node_inputs,
        &lane_label_probe_ids,
        measurements,
    );

    out.push_str("#align(center, swimlane-layout(\n");
    if let Some(t) = &act.title {
        out.push_str("  title: [");
        out.push_str(&typst_escape(t));
        out.push_str("],\n");
    }
    out.push_str("  lanes: (\n");
    for l in &layout.lanes {
        let color = l
            .color
            .as_deref()
            .and_then(puml_color_to_typst)
            .unwrap_or_else(|| "none".to_string());
        let _ = writeln!(
            out,
            "    (label: [{}], color: {}, x: {:.2}pt, width: {:.2}pt),",
            typst_escape(&l.label),
            color,
            l.x_pt,
            l.width_pt,
        );
    }
    out.push_str("  ),\n");
    out.push_str("  nodes: (\n");
    for (i, p) in layout.nodes.iter().enumerate() {
        let n = &nodes[i];
        out.push_str("    (content: ");
        out.push_str(n.typst_expr.trim());
        let _ = writeln!(out, ", x: {:.2}pt, y: {:.2}pt),", p.x_pt, p.y_pt);
    }
    out.push_str("  ),\n");
    out.push_str("  edges: (\n");
    for e in &layout.edges {
        out.push_str("    (points: (");
        for p in &e.points {
            let _ = write!(out, "({:.2}pt, {:.2}pt), ", p.0, p.1);
        }
        let _ = writeln!(out, "), arrow: {}),", e.arrow);
    }
    out.push_str("  ),\n");
    let _ = writeln!(out, "  header-height: {:.2}pt,", layout.header_h_pt);
    let _ = writeln!(out, "  body-height: {:.2}pt,", layout.body_h_pt);
    out.push_str("))\n");
}

/// Legacy grid-based swimlane emitter, retained as the fallback path
/// when the measure pass didn't run (or produced no results). Each
/// lane stays a self-contained `flow-col`; cross-lane connectors are
/// NOT drawn — call the principled `emit_swimlane_layout` instead when
/// a `MeasurementSet` is available.
fn emit_swimlane_lanes_legacy(out: &mut String, body: &[ActivityStmt], indent: usize) {
    let mut lanes: Vec<(Option<String>, Option<String>, Vec<&ActivityStmt>)> = Vec::new();
    // Implicit first lane: collects pre-switch stmts.
    lanes.push((None, None, Vec::new()));
    for s in body {
        if let ActivityStmt::SwimlaneSwitch { label, color, .. } = s {
            // Close the current lane and open a new one. If the implicit
            // first lane is still empty, replace its label/color instead
            // of opening a separate lane.
            if lanes.len() == 1 && lanes[0].2.is_empty() {
                lanes[0].0 = Some(label.clone());
                lanes[0].1 = color.clone();
            } else {
                lanes.push((Some(label.clone()), color.clone(), Vec::new()));
            }
        } else {
            lanes.last_mut().unwrap().2.push(s);
        }
    }
    for (label, color, stmts) in lanes {
        push_indent(out, indent);
        out.push_str("lane([");
        if let Some(l) = label.as_deref() {
            out.push_str(&typst_escape(l));
        }
        out.push_str("], ");
        // Build a flow-col body from the lane's statements.
        let owned: Vec<ActivityStmt> = stmts.iter().map(|s| (*s).clone()).collect();
        emit_flow_col(out, &owned, indent);
        if let Some(c) = color.as_deref().and_then(puml_color_to_typst) {
            out.push_str(", color: ");
            out.push_str(&c);
        }
        out.push_str("),\n");
    }
}

fn emit_stmts(out: &mut String, body: &[ActivityStmt], indent: usize) {
    for s in body {
        push_indent(out, indent);
        emit_stmt(out, s, indent);
        out.push_str(",\n");
    }
}

fn emit_stmt(out: &mut String, s: &ActivityStmt, indent: usize) {
    match s {
        ActivityStmt::Start { .. } => out.push_str("start-marker()"),
        ActivityStmt::Stop { .. } => out.push_str("stop-marker()"),
        ActivityStmt::End { .. } => out.push_str("end-marker()"),
        ActivityStmt::Detach { .. } => out.push_str("detach-marker()"),
        // M0: non-structural escape constructs collapse to a no-op
        // sentinel. `flow-col` simply walks past `[]` content, which
        // keeps the surrounding arrows wiring correctly.
        ActivityStmt::Break { .. }
        | ActivityStmt::GotoLabel { .. }
        | ActivityStmt::Goto { .. }
        | ActivityStmt::SwimlaneSwitch { .. } => out.push_str("[]"),
        ActivityStmt::Action {
            label,
            kind,
            edge_label,
            color,
            notes,
            ..
        } => emit_action(out, label, *kind, edge_label.as_deref(), color.as_deref(), notes),
        ActivityStmt::If {
            cond,
            then_label,
            then_branch,
            elseifs,
            else_label,
            else_branch,
            ..
        } => emit_if(
            out,
            cond,
            then_label.as_deref(),
            then_branch,
            elseifs,
            else_label.as_deref(),
            else_branch.as_deref(),
            indent,
        ),
        ActivityStmt::While {
            cond,
            body,
            is_label,
            ..
        } => emit_while(out, cond, body, is_label.as_deref(), indent),
        ActivityStmt::Repeat {
            body,
            cond,
            is_label,
            ..
        } => emit_repeat(out, body, cond.as_deref(), is_label.as_deref(), indent),
        ActivityStmt::Fork {
            branches, merge, ..
        }
        | ActivityStmt::Split {
            branches, merge, ..
        } => emit_n_way_bar(out, branches, *merge, indent),
        ActivityStmt::Switch { cond, cases, .. } => emit_switch(out, cond, cases, indent),
        ActivityStmt::Partition {
            kind, label, color, body, ..
        } => emit_partition(out, *kind, label, color.as_deref(), body, indent),
    }
}

/// Emit a sequence of statements inside an inline `flow-col(...)` block.
/// Used wherever a Stmt has a child body that must show up as a sub-flow.
fn emit_flow_col(out: &mut String, body: &[ActivityStmt], indent: usize) {
    if body.is_empty() {
        out.push_str("flow-col()");
        return;
    }
    out.push_str("flow-col(\n");
    emit_stmts(out, body, indent + 1);
    push_indent(out, indent);
    out.push(')');
}

fn emit_partition(
    out: &mut String,
    kind: PartitionKind,
    label: &str,
    color: Option<&str>,
    body: &[ActivityStmt],
    indent: usize,
) {
    out.push_str("partition(\n");
    push_indent(out, indent + 1);
    out.push_str("label: [");
    out.push_str(&typst_escape(label));
    out.push_str("],\n");
    push_indent(out, indent + 1);
    out.push_str("kind: \"");
    out.push_str(partition_kind_keyword(kind));
    out.push_str("\",\n");
    if let Some(c) = color.and_then(puml_color_to_typst) {
        push_indent(out, indent + 1);
        out.push_str("color: ");
        out.push_str(&c);
        out.push_str(",\n");
    }
    push_indent(out, indent + 1);
    emit_flow_col(out, body, indent + 1);
    out.push_str(",\n");
    push_indent(out, indent);
    out.push(')');
}

fn partition_kind_keyword(kind: PartitionKind) -> &'static str {
    match kind {
        PartitionKind::Partition => "partition",
        PartitionKind::Package => "package",
        PartitionKind::Rectangle => "rectangle",
        PartitionKind::Card => "card",
        PartitionKind::Group => "group",
    }
}

fn emit_action(
    out: &mut String,
    label: &[String],
    kind: ActionKind,
    edge_label: Option<&str>,
    _color: Option<&str>,
    notes: &[NoteAttach],
) {
    // PlantUML lets `:` actions span multiple lines; we join with the
    // Typst hard line break `\` so blockcell's `process[…]` renders them
    // on separate lines.
    let body = join_typst_label(label);
    let shape = action_kind_shape(kind);

    if notes.is_empty() {
        // Common case: plain `process[…]`, optionally with a shape
        // selector and / or an edge label.
        let mut named: Vec<String> = Vec::new();
        if let Some(s) = shape {
            named.push(format!("shape: \"{s}\""));
        }
        if let Some(arrow) = edge_label {
            named.push(format!("edge-label: [{}]", typst_escape(arrow)));
        }
        if named.is_empty() {
            out.push_str("process[");
        } else {
            out.push_str("process(");
            out.push_str(&named.join(", "));
            out.push_str(")[");
        }
        out.push_str(&body);
        out.push(']');
        return;
    }

    // Action has notes — wrap the host node with `with-notes(...)` so
    // the sticky-note rectangles sit beside it. Notes are partitioned
    // into left / right arrays in source order.
    out.push_str("with-notes(\n  ");
    if let Some(s) = shape {
        out.push_str(&format!("process(shape: \"{s}\")["));
    } else {
        out.push_str("process[");
    }
    out.push_str(&body);
    out.push_str("],\n");
    let left: Vec<&NoteAttach> = notes
        .iter()
        .filter(|n| n.position == NotePosition::LeftOf)
        .collect();
    let right: Vec<&NoteAttach> = notes
        .iter()
        .filter(|n| n.position != NotePosition::LeftOf)
        .collect();
    if !left.is_empty() {
        out.push_str("  left-notes: (");
        for n in &left {
            emit_note(out, n);
            out.push_str(", ");
        }
        out.push_str("),\n");
    }
    if !right.is_empty() {
        out.push_str("  right-notes: (");
        for n in &right {
            emit_note(out, n);
            out.push_str(", ");
        }
        out.push_str("),\n");
    }
    if let Some(arrow) = edge_label {
        out.push_str("  edge-label: [");
        out.push_str(&typst_escape(arrow));
        out.push_str("],\n");
    }
    out.push(')');
}

fn emit_note(out: &mut String, n: &NoteAttach) {
    out.push_str("flow-note([");
    out.push_str(&join_typst_label(&n.text));
    out.push_str("])");
}

/// Map an `ActionKind` to a `flow-node` shape string. Returns `None`
/// when the kind is the default rectangle (so codegen uses the
/// `process[…]` shorthand). M3 partial: only the four most-used SDL /
/// UML signal shapes have dedicated painters; the rest fall back to
/// rectangle.
fn action_kind_shape(kind: ActionKind) -> Option<&'static str> {
    match kind {
        ActionKind::Rectangle => None,
        ActionKind::Input => Some("input"),
        ActionKind::Output => Some("output"),
        ActionKind::SendSignal => Some("sendSignal"),
        ActionKind::ReceiveSignal | ActionKind::AcceptEvent => Some("acceptEvent"),
        // The remaining variants ship as plain rectangles until their
        // painters land.
        _ => None,
    }
}

fn emit_if(
    out: &mut String,
    cond: &str,
    then_label: Option<&str>,
    then_b: &[ActivityStmt],
    elseifs: &[ElseIfBranch],
    else_label: Option<&str>,
    else_b: Option<&[ActivityStmt]>,
    indent: usize,
) {
    if elseifs.is_empty() {
        // Two-arm if: render with branch-merge for the standard
        // diamond-with-rejoining-arms visual. The author-supplied
        // `then (yes)` / `else (no)` clauses become the yes-label /
        // no-label slots on the diamond arrows; absent ones fall back
        // to the painter's defaults ("Yes" / "No").
        //
        // When the else arm is missing OR empty (`else` followed
        // immediately by `endif`), the no-arm becomes a *bypass*: a
        // skip arrow that runs from the diamond's other exit down to
        // the merge line, matching PlantUML's `if (c) then (label) body
        // endif` rendering.
        let else_is_empty = else_b.map_or(true, |b| b.is_empty());

        out.push_str("branch-merge([");
        out.push_str(&typst_escape(cond));
        out.push_str("],\n");
        push_indent(out, indent + 1);
        out.push_str("yes: ");
        emit_flow_col(out, then_b, indent + 1);
        out.push_str(",\n");

        if else_is_empty {
            push_indent(out, indent + 1);
            out.push_str("no: [],\n");
            push_indent(out, indent + 1);
            out.push_str("no-bypass: true,\n");
        } else if let Some(eb) = else_b {
            push_indent(out, indent + 1);
            out.push_str("no: ");
            emit_flow_col(out, eb, indent + 1);
            out.push_str(",\n");
        }

        if let Some(l) = then_label {
            push_indent(out, indent + 1);
            out.push_str("yes-label: [");
            out.push_str(&typst_escape(l));
            out.push_str("],\n");
        }
        if let Some(l) = else_label {
            push_indent(out, indent + 1);
            out.push_str("no-label: [");
            out.push_str(&typst_escape(l));
            out.push_str("],\n");
        }
        if branch_terminates(then_b) {
            push_indent(out, indent + 1);
            out.push_str("yes-detach: true,\n");
        }
        if let Some(eb) = else_b {
            if !eb.is_empty() && branch_terminates(eb) {
                push_indent(out, indent + 1);
                out.push_str("no-detach: true,\n");
            }
        }
        push_indent(out, indent);
        out.push(')');
        return;
    }

    // N-way: render as `switch(...)` so each branch gets its own
    // labelled arm under the diamond. The first arm carries the
    // `then (label)` clause; each elseif uses its `(label)` if set
    // (otherwise the bare condition text); the final `else (label)`
    // becomes the last arm.
    out.push_str("switch([");
    out.push_str(&typst_escape(cond));
    out.push_str("],\n");

    emit_case(
        out,
        then_label.unwrap_or("yes"),
        then_b,
        branch_terminates(then_b),
        indent + 1,
    );

    for ei in elseifs {
        let label_owned: String;
        let label: &str = if let Some(l) = ei.label.as_deref() {
            l
        } else {
            label_owned = format!("{} ?", &ei.cond);
            &label_owned
        };
        emit_case(out, label, &ei.branch, branch_terminates(&ei.branch), indent + 1);
    }
    if let Some(eb) = else_b {
        emit_case(
            out,
            else_label.unwrap_or("no"),
            eb,
            branch_terminates(eb),
            indent + 1,
        );
    }
    push_indent(out, indent);
    out.push(')');
}

fn emit_case(
    out: &mut String,
    label: &str,
    branch: &[ActivityStmt],
    detach: bool,
    indent: usize,
) {
    push_indent(out, indent);
    out.push_str("case([");
    out.push_str(&typst_escape(label));
    out.push_str("], ");
    emit_flow_col(out, branch, indent);
    if detach {
        out.push_str(", detach: true");
    }
    out.push_str("),\n");
}

/// Return `true` when this body's last statement unconditionally exits
/// the surrounding flow (so the enclosing diamond/bar shouldn't draw a
/// rejoin connector). Matches PlantUML's behaviour where a branch
/// ending in `stop`/`end`/`detach`/`kill` doesn't loop back to the
/// outer flow.
fn branch_terminates(body: &[ActivityStmt]) -> bool {
    matches!(
        body.last(),
        Some(ActivityStmt::Stop { .. })
            | Some(ActivityStmt::End { .. })
            | Some(ActivityStmt::Detach { .. })
    )
}

fn emit_while(
    out: &mut String,
    cond: &str,
    body: &[ActivityStmt],
    is_label: Option<&str>,
    indent: usize,
) {
    out.push_str("flow-loop(\n");
    push_indent(out, indent + 1);
    out.push_str("flow-col(\n");
    push_indent(out, indent + 2);
    out.push_str("decision[");
    out.push_str(&typst_escape(cond));
    out.push_str("],\n");
    emit_stmts(out, body, indent + 2);
    push_indent(out, indent + 1);
    out.push_str("),\n");
    push_indent(out, indent + 1);
    out.push_str("back-label: [");
    out.push_str(&typst_escape(is_label.unwrap_or("yes")));
    out.push_str("],\n");
    push_indent(out, indent);
    out.push(')');
}

fn emit_repeat(
    out: &mut String,
    body: &[ActivityStmt],
    cond: Option<&str>,
    is_label: Option<&str>,
    indent: usize,
) {
    out.push_str("flow-loop(\n");
    push_indent(out, indent + 1);
    out.push_str("flow-col(\n");
    emit_stmts(out, body, indent + 2);
    if let Some(c) = cond {
        push_indent(out, indent + 2);
        out.push_str("decision[");
        out.push_str(&typst_escape(c));
        out.push_str("],\n");
    }
    push_indent(out, indent + 1);
    out.push_str("),\n");
    push_indent(out, indent + 1);
    out.push_str("back-label: [");
    out.push_str(&typst_escape(is_label.unwrap_or("repeat")));
    out.push_str("],\n");
    push_indent(out, indent);
    out.push(')');
}

fn emit_n_way_bar(out: &mut String, branches: &[Vec<ActivityStmt>], merge: bool, indent: usize) {
    out.push_str("fork-bar(\n");
    for br in branches {
        emit_case(out, "", br, branch_terminates(br), indent + 1);
    }
    if !merge {
        push_indent(out, indent + 1);
        out.push_str("merge: false,\n");
    }
    push_indent(out, indent);
    out.push(')');
}

fn emit_switch(out: &mut String, cond: &str, cases: &[SwitchCase], indent: usize) {
    out.push_str("switch([");
    out.push_str(&typst_escape(cond));
    out.push_str("],\n");
    for c in cases {
        emit_case(out, &c.value, &c.branch, branch_terminates(&c.branch), indent + 1);
    }
    push_indent(out, indent);
    out.push(')');
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn emit_title(out: &mut String, title: &str) {
    out.push_str("#align(center)[*");
    out.push_str(&typst_escape(title));
    out.push_str("*]\n\n");
}

fn emit_skinparam_preamble(out: &mut String, params: &[Skinparam]) {
    let mut text_args: Vec<String> = Vec::new();
    let mut page_fill: Option<String> = None;
    for p in params {
        match p.key.as_str() {
            "backgroundColor" | "BackgroundColor" => {
                if let Some(c) = puml_color_to_typst(&p.value) {
                    page_fill = Some(c);
                }
            }
            "defaultFontName" | "DefaultFontName" | "defaultFontFamily" => {
                let trimmed = p.value.trim_matches('"');
                if !trimmed.is_empty() {
                    text_args.push(format!("font: \"{}\"", typst_str_escape(trimmed)));
                }
            }
            "defaultFontSize" | "DefaultFontSize" => {
                if let Ok(pt) = p.value.trim().parse::<u32>() {
                    text_args.push(format!("size: {pt}pt"));
                }
            }
            _ => {}
        }
    }
    let had_page_fill = page_fill.is_some();
    if let Some(c) = page_fill {
        let _ = writeln!(out, "#set page(fill: {c})");
    }
    if !text_args.is_empty() {
        let _ = writeln!(out, "#set text({})", text_args.join(", "));
    }
    if had_page_fill || !text_args.is_empty() {
        out.push('\n');
    }
}

fn puml_color_to_typst(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let hex = s.strip_prefix('#').unwrap_or(s);
    let lower = hex.to_ascii_lowercase();
    let named = match lower.as_str() {
        "red" => Some("FF0000"),
        "blue" => Some("0000FF"),
        "green" => Some("008000"),
        "yellow" => Some("FFFF00"),
        "orange" => Some("FFA500"),
        "black" => Some("000000"),
        "white" => Some("FFFFFF"),
        "gray" | "grey" => Some("808080"),
        "lightblue" => Some("ADD8E6"),
        "lightgreen" => Some("90EE90"),
        "lightyellow" => Some("FFFFE0"),
        "lightgray" | "lightgrey" => Some("D3D3D3"),
        _ => None,
    };
    let final_hex = match named {
        Some(h) => h.to_string(),
        None => {
            if hex.chars().all(|c| c.is_ascii_hexdigit()) && (hex.len() == 3 || hex.len() == 6) {
                hex.to_string()
            } else {
                return None;
            }
        }
    };
    Some(format!("rgb(\"#{}\")", final_hex))
}

/// Join a multi-line label using Typst's hard-line-break marker. Lines
/// are individually escaped so user content can't smuggle markup.
fn join_typst_label(lines: &[String]) -> String {
    let mut out = String::new();
    for (i, ln) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str(" \\ ");
        }
        out.push_str(&typst_escape(ln));
    }
    out
}

fn typst_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('#', "\\#")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('`', "\\`")
}

fn typst_str_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ActionKind, ActivityStmt};

    fn action(label: &str) -> ActivityStmt {
        ActivityStmt::Action {
            label: vec![label.to_string()],
            kind: ActionKind::Rectangle,
            color: None,
            url: None,
            notes: Vec::new(),
            edge_label: None,
            line: 1,
        }
    }

    #[test]
    fn linear_emits_flow_col() {
        let mut out = String::new();
        emit(
            &mut out,
            &ActivityDiagram {
                body: vec![
                    ActivityStmt::Start { line: 1 },
                    action("hello"),
                    ActivityStmt::Stop { line: 1 },
                ],
                ..Default::default()
            },
            None,
            0,
        );
        assert!(out.contains("start-marker()"));
        assert!(out.contains("process[hello]"));
        assert!(out.contains("stop-marker()"));
        assert!(out.contains("flow-col"));
    }

    #[test]
    fn two_arm_if_renders_branch_merge() {
        let mut out = String::new();
        emit(
            &mut out,
            &ActivityDiagram {
                body: vec![ActivityStmt::If {
                    cond: "ok?".into(),
                    then_label: None,
                    then_branch: vec![action("A")],
                    elseifs: vec![],
                    else_label: None,
                    else_branch: Some(vec![action("B")]),
                    line: 1,
                }],
                ..Default::default()
            },
            None,
            0,
        );
        assert!(out.contains("branch-merge([ok?]"));
        assert!(out.contains("yes: flow-col"));
        assert!(out.contains("no: flow-col"));
    }

    #[test]
    fn elseif_renders_switch() {
        let mut out = String::new();
        emit(
            &mut out,
            &ActivityDiagram {
                body: vec![ActivityStmt::If {
                    cond: "k".into(),
                    then_label: None,
                    then_branch: vec![action("A")],
                    elseifs: vec![ElseIfBranch {
                        cond: "b".into(),
                        label: None,
                        branch: vec![action("B")],
                        line: 2,
                    }],
                    else_label: None,
                    else_branch: Some(vec![action("C")]),
                    line: 1,
                }],
                ..Default::default()
            },
            None,
            0,
        );
        assert!(out.contains("switch([k]"));
        assert!(out.contains("case([yes]"));
        assert!(out.contains("case([b ?]"));
        assert!(out.contains("case([no]"));
    }

    #[test]
    fn fork_renders_fork_bar() {
        let mut out = String::new();
        emit(
            &mut out,
            &ActivityDiagram {
                body: vec![ActivityStmt::Fork {
                    branches: vec![vec![action("A")], vec![action("B")]],
                    merge: true,
                    line: 1,
                }],
                ..Default::default()
            },
            None,
            0,
        );
        assert!(out.contains("fork-bar"));
        assert!(out.contains("case([], flow-col"));
    }
}
