//! End-to-end CLI tests for use case diagrams. Implemented as a flavor
//! of the cuca (description-family) pipeline: actor / usecase entities
//! share `cuca-layout` with class/component, but PlantUML semantics for
//! `:User:` actor shorthand inside relations, `(UC)` usecase shorthand
//! inside relations, and `<<include>>` / `<<extend>>` stereotype-driven
//! dashed lines are unique to this flavor.

mod common;

use assert_cmd::Command;
use common::{assert_golden_in, emit_typst_path, fixture_in, svg_viewbox_width};

fn golden_usecase(name: &str) {
    let actual = emit_typst_path(&fixture_in("usecase", &format!("{name}.puml")));
    assert_golden_in("usecase", name, &actual);
}

fn render_usecase_svg(name: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join(format!("usecase-{name}.svg"));
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("usecase", &format!("{name}.puml")))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(
        svg.starts_with("<svg") || svg.starts_with("<?xml"),
        "usecase {name} render did not produce SVG"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(width > 50.0, "usecase {name} viewBox suspiciously narrow: {width}");
}

#[test]
fn golden_emit_typst_usecase_basic() { golden_usecase("basic"); }

#[test]
fn golden_emit_typst_usecase_shorthand_top_level() { golden_usecase("shorthand-top-level"); }

#[test]
fn golden_emit_typst_usecase_shorthand_inline() { golden_usecase("shorthand-inline"); }

#[test]
fn renders_svg_for_usecase_shorthand_inline() { render_usecase_svg("shorthand-inline"); }

#[test]
fn golden_emit_typst_usecase_include_extend() { golden_usecase("include-extend"); }

#[test]
fn renders_svg_for_usecase_include_extend() { render_usecase_svg("include-extend"); }

#[test]
fn golden_emit_typst_usecase_direction_lr() { golden_usecase("direction-lr"); }

#[test]
fn golden_emit_typst_usecase_system_boundary() { golden_usecase("system-boundary"); }

#[test]
fn golden_emit_typst_usecase_actor_generalization() { golden_usecase("actor-generalization"); }

#[test]
fn golden_emit_typst_usecase_notes() { golden_usecase("notes"); }

#[test]
fn renders_svg_for_usecase_basic() { render_usecase_svg("basic"); }

#[test]
fn renders_svg_for_usecase_system_boundary() { render_usecase_svg("system-boundary"); }

#[test]
fn renders_svg_for_usecase_notes() { render_usecase_svg("notes"); }
