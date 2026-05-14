//! Shared helpers for the integration test crates.
//!
//! Each `tests/*.rs` file is its own test binary; this module is pulled
//! in with `mod common;` and is *not* compiled as a standalone test
//! crate. `#![allow(dead_code)]` because no single test file exercises
//! every helper.

#![allow(dead_code)]

use assert_cmd::Command;
use std::path::{Path, PathBuf};

/// Path to a fixture at `tests/fixtures/<subdir>/<name>`.
pub fn fixture_in(subdir: &str, name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(subdir);
    p.push(name);
    p
}

/// The third number of the SVG `viewBox` attribute — the page width.
///
/// The viewBox-width assertions across these tests are deliberate: an
/// earlier codegen had a page-width / `seq-puml(width: auto)` interaction
/// that collapsed every diagram into a ~100pt column. Asserting a sensible
/// minimum width makes that failure mode regress loudly.
pub fn svg_viewbox_width(svg: &str) -> Option<f64> {
    let start = svg.find("viewBox=\"")? + "viewBox=\"".len();
    let end = svg[start..].find('"')? + start;
    svg[start..end]
        .split_whitespace()
        .nth(2)
        .and_then(|s| s.parse().ok())
}

/// Compare `actual` against `tests/golden/<subdir>/<name>.typ.golden`.
/// Set `UPDATE_GOLDEN=1` to refresh the snapshot instead of asserting.
pub fn assert_golden_in(subdir: &str, name: &str, actual: &str) {
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

/// Run `typstuml emit <path>` and return its stdout.
pub fn emit_typst_path(path: &Path) -> String {
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
