//! End-to-end smoke tests for the M0 pipeline.
//!
//! These run the actual CLI binary via `assert_cmd`. They exercise the full
//! parse → preprocess → codegen → typst-compile → encode chain, so they
//! double as a regression net for vendoring drift in `assets/blockcell/`.
//!
//! The viewBox-width assertion is deliberate: the previous codegen had a
//! page-width / `seq-puml(width: auto)` interaction that collapsed every
//! diagram into a ~100pt column. Asserting a sensible minimum width here
//! makes that failure mode regress loudly instead of silently.

use assert_cmd::Command;
use predicates::str::contains;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/sequence");
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
