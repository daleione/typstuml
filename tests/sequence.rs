//! End-to-end CLI tests for sequence diagrams (`@startuml` with messages).
//!
//! These run the actual binary via `assert_cmd`, exercising the full
//! parse → preprocess → codegen → typst-compile → encode chain — they
//! double as a regression net for vendoring drift in `components/`.
//!
//! Golden snapshots for the `emit` subcommand live under
//! `tests/golden/sequence/`; refresh with `UPDATE_GOLDEN=1 cargo test`.

mod common;

use assert_cmd::Command;
use common::{assert_golden_in, svg_viewbox_width};
use predicates::str::contains;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/sequence");
    p.push(name);
    p
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
fn golden_emit_typst_self_message() {
    assert_golden("self-message", &emit_typst("self-message.puml"));
}

#[test]
fn golden_emit_typst_activate() {
    assert_golden("activate", &emit_typst("activate.puml"));
}

#[test]
fn golden_emit_typst_alt() {
    assert_golden("alt", &emit_typst("alt.puml"));
}

#[test]
fn golden_emit_typst_opt() {
    assert_golden("opt", &emit_typst("opt.puml"));
}

#[test]
fn golden_emit_typst_loop() {
    assert_golden("loop", &emit_typst("loop.puml"));
}

#[test]
fn golden_emit_typst_par() {
    assert_golden("par", &emit_typst("par.puml"));
}

#[test]
fn golden_emit_typst_group() {
    assert_golden("group", &emit_typst("group.puml"));
}

#[test]
fn golden_emit_typst_autonumber() {
    assert_golden("autonumber", &emit_typst("autonumber.puml"));
}

#[test]
fn golden_emit_typst_create_destroy() {
    assert_golden("create-destroy", &emit_typst("create-destroy.puml"));
}

#[test]
fn golden_emit_typst_divider() {
    assert_golden("divider", &emit_typst("divider.puml"));
}

#[test]
fn golden_emit_typst_arrow_styles() {
    assert_golden("arrow-styles", &emit_typst("arrow-styles.puml"));
}

#[test]
fn golden_emit_typst_return() {
    assert_golden("return", &emit_typst("return.puml"));
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

/// Non-class diagrams (sequence/json/yaml/tree) don't have probes today.
/// Their emit output must be byte-identical with and without
/// `--no-measure` — otherwise we're paying for a pass-1 round trip that
/// has nothing to measure.
#[test]
fn measure_is_noop_for_sequence() {
    let measured = emit_typst("hello.puml");
    let nomeasure_bytes = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg("--no-measure")
        .arg(fixture("hello.puml"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let nomeasure = String::from_utf8(nomeasure_bytes).expect("emit utf8");
    assert_eq!(measured, nomeasure);
}
