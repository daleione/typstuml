//! End-to-end CLI tests for JSON / YAML diagrams. Both flatten to the
//! same record-graph renderer (`components/src/records.typ`), so
//! they're exercised together here.

mod common;

use assert_cmd::Command;
use common::{assert_golden_in, emit_typst_path, fixture_in, svg_viewbox_width};
use predicates::str::contains;

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
fn golden_emit_typst_json_array_of_scalars() {
    let actual = emit_typst_path(&fixture_in("json", "array-of-scalars.puml"));
    assert_golden_in("json", "array-of-scalars", &actual);
}

#[test]
fn golden_emit_typst_json_nested_object() {
    let actual = emit_typst_path(&fixture_in("json", "nested-object.puml"));
    assert_golden_in("json", "nested-object", &actual);
}

#[test]
fn golden_emit_typst_json_null_empty() {
    let actual = emit_typst_path(&fixture_in("json", "null-empty.puml"));
    assert_golden_in("json", "null-empty", &actual);
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
fn golden_emit_typst_yaml_list_of_strings() {
    let actual = emit_typst_path(&fixture_in("yaml", "list-of-strings.puml"));
    assert_golden_in("yaml", "list-of-strings", &actual);
}

#[test]
fn golden_emit_typst_yaml_nested_map() {
    let actual = emit_typst_path(&fixture_in("yaml", "nested-map.puml"));
    assert_golden_in("yaml", "nested-map", &actual);
}

#[test]
fn golden_emit_typst_yaml_mixed_types() {
    let actual = emit_typst_path(&fixture_in("yaml", "mixed-types.puml"));
    assert_golden_in("yaml", "mixed-types", &actual);
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
fn record_graph_measure_changes_emit_output() {
    let measured = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg(fixture_in("json", "person.puml"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let measured = String::from_utf8(measured).unwrap();

    let nomeasure = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg("--no-measure")
        .arg(fixture_in("json", "person.puml"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let nomeasure = String::from_utf8(nomeasure).unwrap();

    assert_ne!(
        measured, nomeasure,
        "measure pass should produce different record positions than the heuristic",
    );
    assert!(measured.contains("record-layout"));
    assert!(nomeasure.contains("record-layout"));
}
