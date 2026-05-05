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
//! Golden snapshots for `--emit-typst` live under `tests/golden/sequence/`.
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
        .arg("--check")
        .arg(fixture("hello.puml"))
        .assert()
        .success()
        .stderr(contains("parse OK"));
}

#[test]
fn emit_typst_includes_seq_puml_call() {
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("--emit-typst")
        .arg("--stdout")
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
        .arg("-o")
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
        .arg("-o")
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
        .arg("--emit-typst")
        .arg("--stdout")
        .arg(fixture(fixture_name))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("emit-typst output is UTF-8")
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
        .arg("--emit-typst")
        .arg("--stdout")
        .arg(path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).expect("emit-typst output is UTF-8")
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
        .arg("-o")
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
        .arg("-o")
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
fn json_strict_rejects_invalid() {
    let tmp = tempfile::tempdir().unwrap();
    let bad = tmp.path().join("bad.puml");
    std::fs::write(&bad, "@startjson\n{\n  \"k\":,\n}\n@endjson\n").unwrap();
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("--check")
        .arg(&bad)
        .assert()
        .failure()
        .stderr(contains("invalid JSON"));
}

#[test]
fn unsupported_diagram_in_strict_mode_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let mindmap = tmp.path().join("mind.puml");
    std::fs::write(&mindmap, "@startmindmap\n* root\n** child\n@endmindmap\n").unwrap();
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg("--check")
        .arg("--compat")
        .arg("strict")
        .arg(&mindmap)
        .assert()
        .failure();
}
