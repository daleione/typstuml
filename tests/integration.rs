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
fn golden_emit_typst_class_cross_cluster() {
    // M4 regression: edge A → B with a third sibling cluster `Middle`
    // between them. The pathplan obstacle list must contain `Middle`'s
    // bbox so the route detours instead of clipping through.
    let actual = emit_typst_path(&fixture_in("class", "cross-cluster.puml"));
    assert_golden_in("class", "cross-cluster", &actual);
}

#[test]
fn golden_emit_typst_class_cluster_rank_order() {
    // M3 regression: PkgA is declared before PkgB but the edge runs
    // Bar → Foo (B → A). With cluster-to-cluster edges participating
    // in Sugiyama ranking, PkgB must end up above PkgA in TB —
    // declaration order alone is not the tiebreaker once an edge
    // exists.
    let actual = emit_typst_path(&fixture_in("class", "cluster-rank-order.puml"));
    assert_golden_in("class", "cluster-rank-order", &actual);
}

#[test]
fn golden_emit_typst_class_sibling_reorder() {
    // M3 barycenter reorder: three top-level packages PkgA / PkgB /
    // PkgC declared in that order feed matching SinkX / SinkY / SinkZ
    // with the deliberately reversed mapping A→Z, B→Y, C→X. The
    // barycenter pass must re-sort row 0 to [PkgC, PkgB, PkgA] so
    // the edges run straight down with zero crossings.
    let actual = emit_typst_path(&fixture_in("class", "sibling-reorder.puml"));
    assert_golden_in("class", "sibling-reorder", &actual);
}

#[test]
fn golden_emit_typst_class_nested_3() {
    // M3 regression: 3 levels of nested packages each holding a
    // direct entity. Inner cluster bboxes must sit inside their
    // parent and the edges must rank entities top → mid → leaf in
    // declaration depth order.
    let actual = emit_typst_path(&fixture_in("class", "nested-3.puml"));
    assert_golden_in("class", "nested-3", &actual);
}

#[test]
fn golden_emit_typst_class_transparent_ancestor() {
    // M3 regression: PkgA is a shared ancestor of edge X --> Y, so it
    // must be transparent for routing (the edge stays inside).
    // Sibling PkgB (containing Z) is opaque — X --> Z must detour
    // around PkgB instead of clipping through.
    let actual = emit_typst_path(&fixture_in("class", "transparent-ancestor.puml"));
    assert_golden_in("class", "transparent-ancestor", &actual);
}

#[test]
fn golden_emit_typst_class_desc_family() {
    // M5-partial / M6 regression: desc-family leaf keywords
    // (component / actor / usecase / database / node / cloud) and the
    // `[Foo]` / `(Foo)` / `:Foo:` inline shorthand must produce
    // entities with the right USymbol. Painter dispatches to per-shape
    // painters for actor / database / component / node (M5 core); the
    // rest fall back to the compartment painter until their painters
    // land in M8.
    let actual = emit_typst_path(&fixture_in("class", "desc-family.puml"));
    assert_golden_in("class", "desc-family", &actual);
}

#[test]
fn golden_emit_typst_class_shapes_desc() {
    // M5 core: dedicated per-USymbol painters for actor / database /
    // component / node. Verifies each gets its own `kind:` keyword in
    // the emit output (not the `class` fallback used for unimplemented
    // shapes like cloud / queue / etc.).
    let actual = emit_typst_path(&fixture_in("class", "shapes-desc.puml"));
    assert_golden_in("class", "shapes-desc", &actual);
}

#[test]
fn renders_svg_for_class_shapes_desc() {
    // End-to-end: the new actor/database/component/node painters must
    // compile through typst-as-library without errors.
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-shapes-desc.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "shapes-desc.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 30,
        "shapes-desc diagram expected many <path>s; got {path_count}"
    );
}

#[test]
fn golden_emit_typst_class_shapes_all() {
    // M5 sweep: actor / usecase / component / database / node / cloud
    // / rectangle / folder / frame / file painters all get used. Each
    // gets its dedicated `kind:` keyword in the emit output (no
    // "class" fallback for these 10 shapes).
    let actual = emit_typst_path(&fixture_in("class", "shapes-all.puml"));
    assert_golden_in("class", "shapes-all", &actual);
}

#[test]
fn renders_svg_for_class_shapes_all() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-shapes-all.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "shapes-all.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 40,
        "shapes-all diagram expected many <path>s; got {path_count}"
    );
}

#[test]
fn golden_emit_typst_class_shapes_deployment() {
    // M5 sweep cont'd: queue / storage / hexagon / card painters.
    let actual = emit_typst_path(&fixture_in("class", "shapes-deployment.puml"));
    assert_golden_in("class", "shapes-deployment", &actual);
}

#[test]
fn golden_emit_typst_class_shapes_activity() {
    // M5 sweep cont'd: artifact / collections / action / process /
    // label painters (the activity-and-flow leaves).
    let actual = emit_typst_path(&fixture_in("class", "shapes-activity.puml"));
    assert_golden_in("class", "shapes-activity", &actual);
}

#[test]
fn renders_svg_for_class_shapes_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-shapes-activity.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "shapes-activity.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
}

#[test]
fn golden_emit_typst_class_sockets() {
    // M7: socket arrow heads `-(` (head_to = SocketOpen) and `)-`
    // (head_from = SocketClosed). The painter renders the head as a
    // half-circle arc whose open side cups the incoming line.
    let actual = emit_typst_path(&fixture_in("class", "sockets.puml"));
    assert_golden_in("class", "sockets", &actual);
}

#[test]
fn renders_svg_for_class_sockets() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-sockets.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "sockets.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
}

#[test]
fn golden_emit_typst_class_shapes_misc() {
    // M5 final sweep: stack / agent / person / boundary / control
    // painters. The entity-domain painter exists in blockcell but
    // needs flavor detection (M5+) to reach it — `entity Foo` in
    // class-flavor still maps to a compartment.
    let actual = emit_typst_path(&fixture_in("class", "shapes-misc.puml"));
    assert_golden_in("class", "shapes-misc", &actual);
}

#[test]
fn renders_svg_for_class_shapes_misc() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-shapes-misc.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "shapes-misc.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
}

#[test]
fn renders_svg_for_class_shapes_deployment() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("class-shapes-deployment.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "shapes-deployment.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
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
fn golden_emit_typst_class_object_basic() {
    let actual = emit_typst_path(&fixture_in("class", "object-basic.puml"));
    assert_golden_in("class", "object-basic", &actual);
}

#[test]
fn renders_svg_for_class_object_basic() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("object-basic.svg");
    Command::cargo_bin("typstuml")
        .unwrap()
        .arg(fixture_in("class", "object-basic.puml"))
        .arg(&out)
        .assert()
        .success();
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"));
    // Object cards should have at least one underline path (the
    // instance-name convention), three field rows for alice, plus
    // edges. A loose floor catches regressions where the object
    // painter renders nothing or collapses to an empty bbox.
    let path_count = svg.matches("<path").count();
    assert!(
        path_count > 10,
        "object diagram expected several <path>s; got {path_count}"
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

// -- Measure protocol -------------------------------------------------------

/// The measure double-pass changes the emitted Typst source: bbox widths
/// and heights come from the painter's `measure()` rather than the Rust
/// heuristic. Disabling it with `--no-measure` must yield a different
/// source (heuristic-derived). If they happen to coincide, the protocol
/// isn't actually engaged — a regression test for "pass-1 is wired up".
#[test]
fn measure_changes_class_emit_output() {
    let path = fixture_in("class", "basic.puml");
    let measured = emit_typst_path(&path);

    let nomeasure = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg("--no-measure")
        .arg(&path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let nomeasure = String::from_utf8(nomeasure).expect("emit utf8");

    assert_ne!(
        measured, nomeasure,
        "measure pass should produce different bbox dimensions than the heuristic; got identical output (protocol may not be engaged)",
    );

    // Both must reference the cuca painter — the protocol must not
    // strip required output.
    assert!(measured.contains("cuca-layout"));
    assert!(nomeasure.contains("cuca-layout"));
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

/// The probe-reported dimensions must equal the painter's actual
/// rendered size to within 0.5pt — this is the §8.2 contract. We
/// verify it indirectly: pass-2 emits `width: {x}pt, height: {y}pt`
/// fixed values, and the painter (class-layout) honors them. So if
/// pass-1 measured value X, pass-2 forwards X, painter renders box
/// of size X. The risk this protects against is codegen forwarding
/// the wrong value (e.g. rounding or unit-conversion bugs between
/// MeasurementSet and the emitted Typst).
#[test]
fn measure_widths_are_forwarded_into_emit() {
    let src = emit_typst_path(&fixture_in("class", "basic.puml"));
    // basic.puml: Animal, Dog, Cat — three classes. Each must appear
    // with both width and height set to a non-default value (i.e.
    // codegen used the measurement, not "auto" or a placeholder).
    for name in ["Animal", "Dog", "Cat"] {
        let line = src
            .lines()
            .find(|l| l.contains(&format!("name: [{name}]")))
            .unwrap_or_else(|| panic!("no class line for {name}"));
        assert!(
            line.contains("width: ") && line.contains("height: "),
            "class {name} line missing width/height: {line}",
        );
        // Heuristic produced multiples of 0.5pt (em factor × FONT_PT,
        // FONT_PT = 10.0). Measured values rarely land on those — so a
        // value with two non-zero decimal digits is a positive sign
        // the measurement actually went through. Specific to basic.puml.
        let has_decimal = line
            .split("width: ")
            .nth(1)
            .and_then(|s| s.split("pt").next())
            .map(|w| w.contains('.') && !w.ends_with(".00"))
            .unwrap_or(false);
        assert!(
            has_decimal,
            "class {name} width looks like a heuristic round number (likely measure didn't run): {line}",
        );
    }
}

// -- Annotations -----------------------------------------------------------

#[test]
fn golden_emit_typst_class_annotations() {
    let actual = emit_typst_path(&fixture_in("class", "annotations.puml"));
    assert_golden_in("class", "annotations", &actual);
}

#[test]
fn annotation_lines_do_not_produce_unrecognized_warnings() {
    // `@Entity` etc. used to flow into the catch-all warning path. The
    // annotation parser captures them silently and attaches to the next
    // declaration.
    let output = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("check")
        .arg(fixture_in("class", "annotations.puml"))
        .assert()
        .success()
        .get_output()
        .stderr
        .clone();
    let stderr = String::from_utf8(output).expect("stderr utf8");
    assert!(
        !stderr.contains("unrecognized class syntax"),
        "annotations should not trigger the unrecognized-syntax diagnostic: {stderr}",
    );
}

#[test]
fn annotation_emit_attaches_to_stereotype_and_member_body() {
    let src = emit_typst_path(&fixture_in("class", "annotations.puml"));
    // Order class stereotype should carry both Entity and Table(...).
    let order_line = src
        .lines()
        .find(|l| l.contains("name: [Order]"))
        .expect("Order line");
    assert!(
        order_line.contains("stereotype: ["),
        "Order should have a stereotype line: {order_line}",
    );
    assert!(
        order_line.contains("Entity") && order_line.contains("Table"),
        "Order stereotype should mention both annotations: {order_line}",
    );
    // Field-level @Id should land in the field body, not the stereotype.
    assert!(
        order_line.contains("body: [Id orderId: Long]")
            || order_line.contains("body: [Id orderId"),
        "@Id should prepend the field body: {order_line}",
    );
}

#[test]
fn package_band_heights_come_from_measurement() {
    // Heuristic CONTAINER_LABEL_PT = 14pt; measured "Domain" at 0.85em
    // of 10pt body font + LABEL_BAND_PADDING_PT = 6pt yields slightly
    // less. Compare two emit runs and confirm the package h: value
    // shifts when --no-measure flips off.
    let measured = emit_typst_path(&fixture_in("class", "package.puml"));
    let nomeasure_bytes = Command::cargo_bin("typstuml")
        .unwrap()
        .arg("emit")
        .arg("--no-measure")
        .arg(fixture_in("class", "package.puml"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let nomeasure = String::from_utf8(nomeasure_bytes).expect("emit utf8");
    fn first_package_height(src: &str) -> &str {
        let line = src
            .lines()
            .find(|l| l.contains("kind: \"package\""))
            .expect("package line");
        let after_h = line.split("h: ").nth(1).expect("h:");
        after_h.split("pt").next().expect("pt unit")
    }
    assert_ne!(
        first_package_height(&measured),
        first_package_height(&nomeasure),
        "measured package band height should differ from heuristic; got measured=\"{}\" nomeasure=\"{}\"",
        first_package_height(&measured),
        first_package_height(&nomeasure),
    );
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

// ---------------------------------------------------------------------------
// Activity diagrams (PlantUML activitydiagram3 / new syntax).
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Use case diagrams. Implemented as a flavor of the cuca (description-family)
// pipeline: actor / usecase entities share `cuca-layout` with class/component,
// but PlantUML semantics for `:User:` actor shorthand inside relations, `(UC)`
// usecase shorthand inside relations, and `<<include>>` / `<<extend>>`
// stereotype-driven dashed lines are unique to this flavor.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// State diagrams (PlantUML UML state machines). S1 scope: simple states,
// pseudostates (initial / final / choice / fork / join), transitions with
// `event [guard] / action` labels, `entry/exit/do` body rows. Codegen emits
// a single `#state-layout(...)` call rendered by `vendor/blockcell/src/states.typ`.
// ---------------------------------------------------------------------------

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
fn renders_svg_for_state_notes() { render_state_svg("notes"); }

#[test]
fn renders_svg_for_state_concurrent_horizontal() { render_state_svg("concurrent-horizontal"); }

#[test]
fn renders_svg_for_state_concurrent_vertical() { render_state_svg("concurrent-vertical"); }

#[test]
fn renders_svg_for_state_note_on_link() { render_state_svg("note-on-link"); }

#[test]
fn renders_svg_for_state_synchro_bar() { render_state_svg("synchro-bar"); }
