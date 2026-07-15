//! End-to-end CLI tests for the tree-shaped diagrams — WBS
//! (`@startwbs`) and mind maps (`@startmindmap`). Both render through
//! `components/src/tree.typ`.

mod common;

use assert_cmd::Command;
use common::{assert_golden_in, emit_typst_path, fixture_in, svg_viewbox_width};

#[test]
fn golden_emit_typst_wbs_basic() {
    let actual = emit_typst_path(&fixture_in("wbs", "basic.puml"));
    assert_golden_in("wbs", "basic", &actual);
}

#[test]
fn golden_emit_typst_wbs_colors() {
    let actual = emit_typst_path(&fixture_in("wbs", "colors.puml"));
    assert_golden_in("wbs", "colors", &actual);
}

#[test]
fn golden_emit_typst_wbs_multiline() {
    let actual = emit_typst_path(&fixture_in("wbs", "multiline.puml"));
    assert_golden_in("wbs", "multiline", &actual);
}

#[test]
fn golden_emit_typst_wbs_pseudo_node() {
    let actual = emit_typst_path(&fixture_in("wbs", "pseudo-node.puml"));
    assert_golden_in("wbs", "pseudo-node", &actual);
}

#[test]
fn renders_svg_for_wbs_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("wbs-basic.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("wbs", "basic.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    // 7 nodes (root + 2 children + 4 leaves) → many <path>s for borders +
    // connectors. Use a low floor to stay tolerant of painter changes.
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 20,
        "WBS render expected many <path>s, got {path_count}"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(width > 200.0, "WBS viewBox unexpectedly small: {width}");
}

#[test]
fn renders_svg_for_wbs_multiline() {
    // Multi-line labels should not crash the painter.
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("wbs-multi.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("wbs", "multiline.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
}

#[test]
fn wbs_strict_rejects_orphan_child() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.puml");
    // depth-3 marker before any root — must fail strict.
    std::fs::write(&bad, "@startwbs\n*** orphan\n@endwbs\n").unwrap();
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("--compat")
        .arg("strict")
        .arg("check")
        .arg(&bad)
        .assert()
        .failure();
}

#[test]
fn golden_emit_typst_mindmap_basic() {
    let actual = emit_typst_path(&fixture_in("mindmap", "basic.puml"));
    assert_golden_in("mindmap", "basic", &actual);
}

#[test]
fn golden_emit_typst_mindmap_orgmode() {
    let actual = emit_typst_path(&fixture_in("mindmap", "orgmode.puml"));
    assert_golden_in("mindmap", "orgmode", &actual);
}

#[test]
fn golden_emit_typst_mindmap_colors() {
    let actual = emit_typst_path(&fixture_in("mindmap", "colors.puml"));
    assert_golden_in("mindmap", "colors", &actual);
}

#[test]
fn golden_emit_typst_mindmap_deep_tree() {
    let actual = emit_typst_path(&fixture_in("mindmap", "deep-tree.puml"));
    assert_golden_in("mindmap", "deep-tree", &actual);
}

#[test]
fn renders_svg_for_mindmap_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("mindmap-basic.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("mindmap", "basic.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 20,
        "mindmap render expected many <path>s, got {path_count}"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    // Mindmap fans out left + right, so it should be visibly wider than a
    // single-column WBS of the same node count.
    assert!(width > 300.0, "mindmap viewBox unexpectedly narrow: {width}");
}

#[test]
fn renders_svg_for_mindmap_colors() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("mindmap-colors.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("mindmap", "colors.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
}

#[test]
fn mindmap_strict_rejects_orphan_child() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.puml");
    std::fs::write(&bad, "@startmindmap\n+++ orphan\n@endmindmap\n").unwrap();
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("--compat")
        .arg("strict")
        .arg("check")
        .arg(&bad)
        .assert()
        .failure();
}
