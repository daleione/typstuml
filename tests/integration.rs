//! End-to-end smoke tests for the CLI pipeline.
//!
//! These run the actual binary via `assert_cmd`. They exercise the full
//! parse → preprocess → codegen → typst-compile → encode chain, so they
//! double as a regression net for vendoring drift in `vendor/blockcell/`.
//!
//! The viewBox-width assertion is deliberate: the previous codegen had a
//! page-width / `seq-puml(width: auto)` interaction that collapsed every
//! diagram into a ~100pt column. Asserting a sensible minimum width here
//! makes that failure mode regress loudly instead of silently.
//!
//! Golden snapshots for the `emit` subcommand live under `tests/golden/sequence/`.
//! Re-generate them with `UPDATE_GOLDEN=1 cargo test`.

use assert_cmd::Command;
use predicates::str::contains;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/sequence");
    p.push(name);
    p
}

fn fixture_in(subdir: &str, name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(subdir);
    p.push(name);
    p
}

fn svg_viewbox_width(svg: &str) -> Option<f64> {
    let start = svg.find("viewBox=\"")? + "viewBox=\"".len();
    let end = svg[start..].find('"')? + start;
    svg[start..end]
        .split_whitespace()
        .nth(2)
        .and_then(|s| s.parse().ok())
}

#[test]
fn check_succeeds_on_hello() {
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("check")
        .arg(fixture("hello.puml"))
        .assert()
        .success()
        .stderr(contains("parse OK"));
}

#[test]
fn emit_typst_includes_seq_puml_call() {
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg(fixture("hello.puml"))
        .assert()
        .success()
        .stdout(contains("seq-puml("));
}

#[test]
fn renders_svg_for_hello_with_real_columns() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("hello.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture("hello.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(
        width > 200.0,
        "diagram column-collapse regression: viewBox width={width}"
    );
}

#[test]
fn renders_svg_for_auth_flow_with_real_columns() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("auth.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture("auth-flow.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(
        width > 350.0,
        "auth-flow expected to be wider than hello: viewBox width={width}"
    );
}

fn emit_typst(fixture_name: &str) -> String {
    let output = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg(fixture(fixture_name))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("emit output is UTF-8")
}

fn assert_golden(name: &str, actual: &str) {
    assert_golden_in("sequence", name, actual);
}

fn assert_golden_in(subdir: &str, name: &str, actual: &str) {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/golden");
    path.push(subdir);
    path.push(format!("{name}.typ.golden"));

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read golden {}: {e}", path.display()));
    assert_eq!(
        actual, expected,
        "golden mismatch for {subdir}/{name}; rerun with UPDATE_GOLDEN=1 to refresh"
    );
}

fn emit_typst_path(path: &std::path::Path) -> String {
    let output = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg(path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("emit output is UTF-8")
}

#[test]
fn golden_emit_typst_hello() {
    assert_golden("hello", &emit_typst("hello.puml"));
}

#[test]
fn golden_emit_typst_auth_flow() {
    assert_golden("auth-flow", &emit_typst("auth-flow.puml"));
}

#[test]
fn golden_emit_typst_styled() {
    assert_golden("styled", &emit_typst("styled.puml"));
}

#[test]
fn skinparam_drives_page_fill_in_svg() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("styled.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture("styled.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    // backgroundColor #F8F8F2 → page fill → SVG <rect> filling the viewBox.
    // We don't pin the exact element shape, but the hex (or rgb()) should
    // appear somewhere in the document.
    let case_insensitive = svg.to_ascii_lowercase();
    assert!(
        case_insensitive.contains("#f8f8f2") || case_insensitive.contains("248"),
        "expected styled background color in SVG output"
    );
}

#[test]
fn golden_emit_typst_json_person() {
    let actual = emit_typst_path(&fixture_in("json", "person.puml"));
    assert_golden_in("json", "person", &actual);
}

#[test]
fn renders_svg_for_json_person() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("person.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("json", "person.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    // Typst rasterizes glyphs to paths, so we can't grep for label text;
    // assert structure instead — the JSON tree should produce many node
    // boxes (rects with stroke) and connector strokes.
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 50,
        "expected JSON tree to emit many <path>s; got {path_count}"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(
        width > 200.0,
        "JSON tree viewBox width unexpectedly small: {width}"
    );
}

#[test]
fn golden_emit_typst_yaml_person() {
    let actual = emit_typst_path(&fixture_in("yaml", "person.puml"));
    assert_golden_in("yaml", "person", &actual);
}

#[test]
fn renders_svg_for_yaml_person() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("person.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("yaml", "person.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 50,
        "expected YAML record-graph to emit many <path>s; got {path_count}"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(
        width > 200.0,
        "YAML record-graph viewBox width unexpectedly small: {width}"
    );
}

#[test]
fn yaml_strict_rejects_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.puml");
    // Tab indentation is invalid in YAML.
    std::fs::write(&bad, "@startyaml\nroot:\n\tchild: 1\n@endyaml\n").unwrap();
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("check")
        .arg(&bad)
        .assert()
        .failure()
        .stderr(contains("invalid YAML"));
}

#[test]
fn json_strict_rejects_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.puml");
    std::fs::write(&bad, "@startjson\n{\n  \"k\":,\n}\n@endjson\n").unwrap();
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("check")
        .arg(&bad)
        .assert()
        .failure()
        .stderr(contains("invalid JSON"));
}

#[test]
fn unsupported_diagram_in_strict_mode_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let salt = tmp.path().join("salt.puml");
    std::fs::write(&salt, "@startsalt\n{\n  Login |\n  Cancel\n}\n@endsalt\n").unwrap();
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("--compat")
        .arg("strict")
        .arg("check")
        .arg(&salt)
        .assert()
        .failure();
}

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

#[test]
fn golden_emit_typst_class_basic() {
    let actual = emit_typst_path(&fixture_in("class", "basic.puml"));
    assert_golden_in("class", "basic", &actual);
}

#[test]
fn golden_emit_typst_class_with_members() {
    let actual = emit_typst_path(&fixture_in("class", "with-members.puml"));
    assert_golden_in("class", "with-members", &actual);
}

#[test]
fn golden_emit_typst_class_heads() {
    let actual = emit_typst_path(&fixture_in("class", "heads.puml"));
    assert_golden_in("class", "heads", &actual);
}

#[test]
fn golden_emit_typst_class_notes() {
    let actual = emit_typst_path(&fixture_in("class", "notes.puml"));
    assert_golden_in("class", "notes", &actual);
}

#[test]
fn golden_emit_typst_class_package() {
    let actual = emit_typst_path(&fixture_in("class", "package.puml"));
    assert_golden_in("class", "package", &actual);
}

#[test]
fn golden_emit_typst_class_hide() {
    let actual = emit_typst_path(&fixture_in("class", "hide.puml"));
    assert_golden_in("class", "hide", &actual);
}

#[test]
fn golden_emit_typst_class_together() {
    let actual = emit_typst_path(&fixture_in("class", "together.puml"));
    assert_golden_in("class", "together", &actual);
}

#[test]
fn golden_emit_typst_class_lollipop() {
    let actual = emit_typst_path(&fixture_in("class", "lollipop.puml"));
    assert_golden_in("class", "lollipop", &actual);
}

#[test]
fn golden_emit_typst_class_assoc() {
    let actual = emit_typst_path(&fixture_in("class", "assoc.puml"));
    assert_golden_in("class", "assoc", &actual);
}

#[test]
fn renders_svg_for_class_hide() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-hide.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "hide.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    // `hide methods` removes the methods compartment — checkout /
    // lineTotal text should not appear in the rendered SVG.
    assert!(!svg.contains("checkout"));
    assert!(!svg.contains("lineTotal"));
    // `hide stereotype` was NOT set, so «aggregate root» should still
    // appear (it's not a stereotype circle, but a stereotype text line).
    // We don't have hide stereotype on this fixture so it stays.
}

#[test]
fn renders_svg_for_class_package() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-package.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "package.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    // Three packages → at least three rectangle outlines on top of the
    // class boxes. Width must accommodate two side-by-side packages.
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    assert!(width > 200.0, "package viewBox unexpectedly small: {width}");
}

#[test]
fn renders_svg_for_class_notes() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-notes.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "notes.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    // Notes draw a polygon with a dog-ear cut + a fold-triangle. Even a
    // single note adds at least two extra <path> elements over the same
    // diagram without notes.
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 20,
        "notes diagram expected many <path>s; got {path_count}"
    );
}

#[test]
fn renders_svg_for_class_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-basic.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "basic.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 30,
        "class diagram expected many <path>s; got {path_count}"
    );
    let width = svg_viewbox_width(&svg).expect("viewBox missing");
    // Three classes (Animal/Dog/Cat) stacked TB → at least one column +
    // one row of labels. Tighter floor than wider record-graph fixtures.
    assert!(width > 150.0, "class viewBox unexpectedly small: {width}");
}

#[test]
fn renders_svg_for_class_with_members() {
    // Members + modifiers + interface stereotype. Smoke test.
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-members.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "with-members.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
}

#[test]
fn class_strict_rejects_unknown_syntax() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.puml");
    std::fs::write(&bad, "@startuml\nclass A\nfrobnicate the foozle\n@enduml\n").unwrap();
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
