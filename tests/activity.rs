//! End-to-end CLI tests for activity diagrams (PlantUML
//! `activitydiagram3` / new syntax).

mod common;

use assert_cmd::Command;
use common::{assert_golden_in, emit_typst_path, fixture_in, svg_viewbox_width};

fn golden_activity(name: &str) {
    let actual = emit_typst_path(&fixture_in("activity", &format!("{name}.puml")));
    assert_golden_in("activity", name, &actual);
}

fn render_activity_svg(name: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join(format!("activity-{name}.svg"));
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("activity", &format!("{name}.puml")))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(
        svg.starts_with("<svg") || svg.starts_with("<?xml"),
        "activity {name} render did not produce SVG"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(width > 50.0, "activity {name} viewBox suspiciously narrow: {width}");
}

#[test]
fn golden_emit_typst_activity_linear() { golden_activity("linear"); }

#[test]
fn golden_emit_typst_activity_if_else() { golden_activity("if-else"); }

#[test]
fn golden_emit_typst_activity_if_elseif() { golden_activity("if-elseif"); }

#[test]
fn golden_emit_typst_activity_while() { golden_activity("while"); }

#[test]
fn golden_emit_typst_activity_repeat() { golden_activity("repeat"); }

#[test]
fn golden_emit_typst_activity_fork() { golden_activity("fork"); }

#[test]
fn golden_emit_typst_activity_split() { golden_activity("split"); }

#[test]
fn golden_emit_typst_activity_switch() { golden_activity("switch"); }

#[test]
fn golden_emit_typst_activity_nested() { golden_activity("nested"); }

#[test]
fn golden_emit_typst_activity_multiline_action() { golden_activity("multiline-action"); }

#[test]
fn golden_emit_typst_activity_partition() { golden_activity("partition"); }

#[test]
fn golden_emit_typst_activity_notes() { golden_activity("notes"); }

#[test]
fn golden_emit_typst_activity_swimlane() { golden_activity("swimlane"); }

#[test]
fn golden_emit_typst_activity_action_shapes() { golden_activity("action-shapes"); }

#[test]
fn golden_emit_typst_activity_if_labels() { golden_activity("if-labels"); }

#[test]
fn golden_emit_typst_activity_empty_else() { golden_activity("empty-else"); }

#[test]
fn renders_svg_for_activity_linear() { render_activity_svg("linear"); }

#[test]
fn renders_svg_for_activity_if_else() { render_activity_svg("if-else"); }

#[test]
fn renders_svg_for_activity_fork() { render_activity_svg("fork"); }

#[test]
fn renders_svg_for_activity_nested() { render_activity_svg("nested"); }

#[test]
fn renders_svg_for_activity_partition() { render_activity_svg("partition"); }

#[test]
fn renders_svg_for_activity_notes() { render_activity_svg("notes"); }

#[test]
fn renders_svg_for_activity_swimlane() { render_activity_svg("swimlane"); }

#[test]
fn renders_svg_for_activity_action_shapes() { render_activity_svg("action-shapes"); }

#[test]
fn renders_svg_for_activity_if_labels() { render_activity_svg("if-labels"); }

#[test]
fn renders_svg_for_activity_empty_else() { render_activity_svg("empty-else"); }

#[test]
fn golden_emit_typst_activity_swimlane_back() { golden_activity("swimlane-back"); }

#[test]
fn renders_svg_for_activity_swimlane_back() { render_activity_svg("swimlane-back"); }

// Lane switch inside a nested if-then: PlantUML drops the inner switch
// today (the whole `if` compound stays in the lane it started in). The
// post-compound cross-lane jump still routes correctly. Captured here
// so a future fix flips the golden visibly.
#[test]
fn golden_emit_typst_activity_swimlane_if() { golden_activity("swimlane-if"); }

#[test]
fn renders_svg_for_activity_swimlane_if() { render_activity_svg("swimlane-if"); }

// Same story for `while`: lane switch inside the loop body is dropped;
// the post-loop revisit consolidates back to the original column.
#[test]
fn golden_emit_typst_activity_swimlane_while() { golden_activity("swimlane-while"); }

#[test]
fn renders_svg_for_activity_swimlane_while() { render_activity_svg("swimlane-while"); }
