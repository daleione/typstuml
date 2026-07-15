//! End-to-end CLI tests for state diagrams (PlantUML UML state
//! machines): simple & composite states, pseudostates (initial / final
//! / choice / fork / join / history / synchro bar / entry / exit),
//! transitions with `event [guard] / action` labels, concurrent regions,
//! and notes. Codegen emits a single `#state-layout(...)` call rendered
//! by `components/src/states.typ`.

mod common;

use assert_cmd::Command;
use common::{assert_golden_in, emit_typst_path, fixture_in, svg_viewbox_width};

fn golden_state(name: &str) {
    let actual = emit_typst_path(&fixture_in("state", &format!("{name}.puml")));
    assert_golden_in("state", name, &actual);
}

fn render_state_svg(name: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join(format!("state-{name}.svg"));
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("state", &format!("{name}.puml")))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(
        svg.starts_with("<svg") || svg.starts_with("<?xml"),
        "state {name} render did not produce SVG"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(width > 50.0, "state {name} viewBox suspiciously narrow: {width}");
}

#[test]
fn golden_emit_typst_state_basic() { golden_state("basic"); }

#[test]
fn golden_emit_typst_state_initial_final() { golden_state("initial-final"); }

#[test]
fn golden_emit_typst_state_choice() { golden_state("choice"); }

#[test]
fn golden_emit_typst_state_fork_join() { golden_state("fork-join"); }

#[test]
fn golden_emit_typst_state_labels_eg_action() { golden_state("labels-eg-action"); }

#[test]
fn golden_emit_typst_state_auto_create() { golden_state("auto-create"); }

// `A -> B` (single dash) is a horizontal link: State1 / State2 must land
// on the same rank, side by side — not stacked vertically.
#[test]
fn golden_emit_typst_state_horizontal() { golden_state("horizontal"); }

#[test]
fn golden_emit_typst_state_colors() { golden_state("colors"); }

#[test]
fn golden_emit_typst_state_multiline_label() { golden_state("multiline-label"); }

#[test]
fn golden_emit_typst_state_direction_lr() { golden_state("direction-lr"); }

#[test]
fn golden_emit_typst_state_composite() { golden_state("composite"); }

#[test]
fn golden_emit_typst_state_composite_nested() { golden_state("composite-nested"); }

#[test]
fn golden_emit_typst_state_cross_composite_cycle() { golden_state("cross-composite-cycle"); }

#[test]
fn golden_emit_typst_state_notes() { golden_state("notes"); }

#[test]
fn golden_emit_typst_state_concurrent_horizontal() { golden_state("concurrent-horizontal"); }

#[test]
fn golden_emit_typst_state_concurrent_vertical() { golden_state("concurrent-vertical"); }

#[test]
fn golden_emit_typst_state_note_on_link() { golden_state("note-on-link"); }

#[test]
fn golden_emit_typst_state_synchro_bar() { golden_state("synchro-bar"); }

#[test]
fn golden_emit_typst_state_floating_note() { golden_state("floating-note"); }

#[test]
fn golden_emit_typst_state_entry_exit() { golden_state("entry-exit"); }

#[test]
fn golden_emit_typst_state_history() { golden_state("history"); }

#[test]
fn golden_emit_typst_state_deep_history() { golden_state("deep-history"); }

#[test]
fn golden_emit_typst_state_self_transition() { golden_state("self-transition"); }

#[test]
fn golden_emit_typst_state_composite_exit_routing() { golden_state("composite-exit-routing"); }

#[test]
fn renders_svg_for_state_composite_exit_routing() { render_state_svg("composite-exit-routing"); }

// `fork1 ---> State1` (3 dashes → minlen 2) must rank State1 one level
// below `fork1 --> State2` (minlen 1); also exercises every pseudostate
// stereotype + label nodes.
#[test]
fn golden_emit_typst_state_stereotype_pseudostates() { golden_state("stereotype-pseudostates"); }

#[test]
fn renders_svg_for_state_stereotype_pseudostates() { render_state_svg("stereotype-pseudostates"); }

#[test]
fn renders_svg_for_state_basic() { render_state_svg("basic"); }

#[test]
fn renders_svg_for_state_initial_final() { render_state_svg("initial-final"); }

#[test]
fn renders_svg_for_state_choice() { render_state_svg("choice"); }

#[test]
fn renders_svg_for_state_fork_join() { render_state_svg("fork-join"); }

#[test]
fn renders_svg_for_state_labels_eg_action() { render_state_svg("labels-eg-action"); }

#[test]
fn renders_svg_for_state_auto_create() { render_state_svg("auto-create"); }

#[test]
fn renders_svg_for_state_horizontal() { render_state_svg("horizontal"); }

#[test]
fn renders_svg_for_state_colors() { render_state_svg("colors"); }

#[test]
fn renders_svg_for_state_multiline_label() { render_state_svg("multiline-label"); }

#[test]
fn renders_svg_for_state_direction_lr() { render_state_svg("direction-lr"); }

#[test]
fn renders_svg_for_state_composite() { render_state_svg("composite"); }

#[test]
fn renders_svg_for_state_composite_nested() { render_state_svg("composite-nested"); }

#[test]
fn renders_svg_for_state_cross_composite_cycle() { render_state_svg("cross-composite-cycle"); }

#[test]
fn renders_svg_for_state_notes() { render_state_svg("notes"); }

#[test]
fn renders_svg_for_state_concurrent_horizontal() { render_state_svg("concurrent-horizontal"); }

#[test]
fn renders_svg_for_state_concurrent_vertical() { render_state_svg("concurrent-vertical"); }

#[test]
fn renders_svg_for_state_note_on_link() { render_state_svg("note-on-link"); }

#[test]
fn renders_svg_for_state_synchro_bar() { render_state_svg("synchro-bar"); }

#[test]
fn renders_svg_for_state_floating_note() { render_state_svg("floating-note"); }

#[test]
fn renders_svg_for_state_entry_exit() { render_state_svg("entry-exit"); }
