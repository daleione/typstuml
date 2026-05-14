//! End-to-end CLI tests for the cuca diagram family — class /
//! component / deployment / object — plus the measure double-pass and
//! the annotation parser. All render through
//! `vendor/blockcell/src/cuca.typ` via `#cuca-layout(...)`.

mod common;

use assert_cmd::Command;
use common::{assert_golden_in, emit_typst_path, fixture_in, svg_viewbox_width};

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

// -- Annotations ------------------------------------------------------------

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
