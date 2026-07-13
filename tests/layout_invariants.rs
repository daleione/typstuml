//! Geometric invariants for cuca component/architecture diagrams.
//!
//! Parses the `.puml` fixture to recover declared package nesting and
//! the `emit` output to recover computed rectangles, then asserts the
//! containment contract that visually broke on
//! `tests/fixtures/component/01-architecture.puml`: every entity must
//! sit fully inside its declared ancestor package frames, never poke
//! into a foreign frame, and never overlap another entity.
//!
//! Entities that are deliberately snapped onto a package boundary
//! (interface/port nodes, from the M5 milestone) are exempted via
//! `SNAPPED_TO_BOUNDARY` — see docs/cuca-architecture-layout-redesign.md §3.2c.

mod common;

use common::{emit_typst_path, fixture_in};
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    /// The emitted `kind: "..."` string, e.g. `"component"`,
    /// `"database"`, `"lollipop"`.
    kind: String,
}

impl Rect {
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// True if the two rects' interiors overlap by more than `eps`.
    fn overlaps(&self, other: &Rect, eps: f64) -> bool {
        let x_overlap = self.x + eps < other.right() - eps && other.x + eps < self.right() - eps;
        let y_overlap = self.y + eps < other.bottom() - eps && other.y + eps < self.bottom() - eps;
        x_overlap && y_overlap
    }

    /// True if `self` sits fully inside `other`, with `eps` slack.
    fn contained_in(&self, other: &Rect, eps: f64) -> bool {
        self.x >= other.x - eps
            && self.y >= other.y - eps
            && self.right() <= other.right() + eps
            && self.bottom() <= other.bottom() + eps
    }
}

/// One declared entity from the `.puml` source, in declaration order,
/// with the chain of package indices (outermost first) it is nested in.
struct DeclaredEntity {
    #[allow(dead_code)]
    id: String,
    ancestors: Vec<usize>,
}

/// One declared package from the `.puml` source, in declaration order.
struct DeclaredPackage {
    #[allow(dead_code)]
    label: String,
    parent: Option<usize>,
}

/// Very small structural scan of a `.puml` fixture: tracks `package
/// "..." { ... }` nesting and records every `component|interface|
/// database|node|folder|frame|cloud "..." as ID` declaration with its
/// enclosing package chain. Good enough for the flat, single-line-per-
/// declaration fixtures under `tests/fixtures/component/`.
fn scan_puml(src: &str) -> (Vec<DeclaredEntity>, Vec<DeclaredPackage>) {
    let mut packages = Vec::new();
    let mut entities = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    let entity_kw = [
        "component", "interface", "database", "node", "folder", "frame", "cloud", "rectangle",
        "queue", "storage", "artifact", "actor", "usecase", "card",
    ];

    for raw_line in src.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('\'') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("package ") {
            if rest.contains('{') {
                let label = rest
                    .split('"')
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
                let parent = stack.last().copied();
                let idx = packages.len();
                packages.push(DeclaredPackage { label, parent });
                stack.push(idx);
            }
            continue;
        }

        if line == "}" {
            stack.pop();
            continue;
        }

        let first_word = line.split_whitespace().next().unwrap_or("");
        if entity_kw.contains(&first_word) {
            let id = if let Some(pos) = line.rfind(" as ") {
                line[pos + 4..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_string()
            } else {
                line.split('"').nth(1).unwrap_or("").to_string()
            };
            entities.push(DeclaredEntity {
                id,
                ancestors: stack.clone(),
            });
        }
    }

    (entities, packages)
}

/// Pull every `(x: ..pt, y: ..pt, width: ..pt, height: ..pt, kind: "..", ...)`
/// tuple out of the `classes: ( ... )` block, in emission order.
fn extract_classes(typ: &str) -> Vec<Rect> {
    extract_block(typ, "classes:", &["x", "y", "width", "height"])
}

/// Pull every `(x: ..pt, y: ..pt, w: ..pt, h: ..pt, kind: "package", ...)`
/// tuple out of the `packages: ( ... )` block, in emission order.
fn extract_packages(typ: &str) -> Vec<Rect> {
    extract_block(typ, "packages:", &["x", "y", "w", "h"])
}

fn extract_block(typ: &str, header: &str, keys: &[&str; 4]) -> Vec<Rect> {
    let Some(start) = typ.find(header) else {
        return Vec::new();
    };
    let rest = &typ[start..];
    let Some(open) = rest.find('(') else {
        return Vec::new();
    };
    // Find the matching close paren for the block, tracking depth.
    let mut depth = 0i32;
    let mut end = None;
    for (i, ch) in rest[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = end else {
        return Vec::new();
    };
    let body = &rest[open + 1..end];

    let mut rects = Vec::new();
    // Each entry is itself a `(...)` tuple; split on top-level entries by
    // tracking paren depth so nested `((c1: ..., ...),)` edge paths (not
    // present here, but future-proofing) don't confuse the split.
    let mut entry_depth = 0i32;
    let mut entry_start = None;
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;
        match ch {
            '(' => {
                if entry_depth == 0 {
                    entry_start = Some(i);
                }
                entry_depth += 1;
            }
            ')' => {
                entry_depth -= 1;
                if entry_depth == 0 {
                    if let Some(s) = entry_start {
                        let entry = &body[s + 1..i];
                        if let Some(r) = parse_rect(entry, keys) {
                            rects.push(r);
                        }
                    }
                    entry_start = None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    rects
}

fn parse_rect(entry: &str, keys: &[&str; 4]) -> Option<Rect> {
    let get = |key: &str| -> Option<f64> {
        let needle = format!("{key}: ");
        let pos = entry.find(&needle)?;
        let after = &entry[pos + needle.len()..];
        let end = after.find(|c: char| c != '-' && c != '.' && !c.is_ascii_digit())?;
        after[..end].parse().ok()
    };
    let w = get(keys[2])?;
    // A lollipop entry (`kind: "lollipop"`) has no emitted `height` —
    // the painter derives it from the disc + label lines painter-side,
    // and its real footprint (a small disc plus a label hanging below)
    // isn't reconstructable from emitted fields alone. Fall back to
    // width so the entry still parses; `kind` lets the caller exempt
    // lollipops from the strict containment/overlap checks instead of
    // asserting on a fabricated height (see the `is_lollipop` skip in
    // the test below).
    let h = get(keys[3]).unwrap_or(w);
    let kind = {
        let needle = "kind: \"";
        entry.find(needle).and_then(|pos| {
            let after = &entry[pos + needle.len()..];
            after.find('"').map(|end| after[..end].to_string())
        })
    }
    .unwrap_or_default();
    Some(Rect {
        x: get(keys[0])?,
        y: get(keys[1])?,
        w,
        h,
        kind,
    })
}

/// Every distinct ancestor-package index found across a declared
/// entity's chain, plus itself if it *is* a package (used to build the
/// "foreign frame" exclusion set for package-vs-package checks).
fn ancestor_set(chain: &[usize]) -> HashSet<usize> {
    chain.iter().copied().collect()
}

#[test]
fn component_architecture_containment_and_overlap() {
    const EPS: f64 = 0.5;

    let fixture_path = fixture_in("component", "01-architecture.puml");
    let src = std::fs::read_to_string(&fixture_path).expect("read fixture");
    let (declared_entities, declared_packages) = scan_puml(&src);
    assert!(
        !declared_entities.is_empty() && !declared_packages.is_empty(),
        "fixture scan found no entities/packages — scan_puml likely drifted from the fixture syntax"
    );

    let typ = emit_typst_path(&fixture_path);
    let entity_rects = extract_classes(&typ);
    let package_rects = extract_packages(&typ);

    assert_eq!(
        entity_rects.len(),
        declared_entities.len(),
        "emitted entity count != declared entity count (order-based zip would misalign)"
    );
    assert_eq!(
        package_rects.len(),
        declared_packages.len(),
        "emitted package count != declared package count"
    );

    // Lollipop discs have no reconstructable height from emitted
    // output (see `parse_rect`) and are, per M5, allowed to snap onto
    // a package boundary by design — exempt them from the strict
    // per-entity checks below (M1 still catches the containment
    // regressions this test exists for; the exempted shapes are a
    // handful of interface markers, not the packages/components that
    // matter for the containment fix).
    let is_lollipop = |r: &Rect| r.kind == "lollipop";

    // (i) No two entities overlap.
    for i in 0..entity_rects.len() {
        if is_lollipop(&entity_rects[i]) {
            continue;
        }
        for j in (i + 1)..entity_rects.len() {
            if is_lollipop(&entity_rects[j]) {
                continue;
            }
            assert!(
                !entity_rects[i].overlaps(&entity_rects[j], EPS),
                "entity {i} ({:?}) overlaps entity {j} ({:?})",
                entity_rects[i],
                entity_rects[j]
            );
        }
    }

    // (ii) Every entity is contained in every declared ancestor frame,
    // and (iii) does not intersect any frame it is not a descendant of.
    for (idx, decl) in declared_entities.iter().enumerate() {
        let rect = &entity_rects[idx];
        if is_lollipop(rect) {
            continue;
        }
        let ancestors = ancestor_set(&decl.ancestors);
        for (pkg_idx, pkg_rect) in package_rects.iter().enumerate() {
            if ancestors.contains(&pkg_idx) {
                assert!(
                    rect.contained_in(pkg_rect, EPS),
                    "entity {idx} (id={}, rect={:?}) is not contained in its declared ancestor package {pkg_idx} ({:?})",
                    decl.id,
                    rect,
                    pkg_rect
                );
            } else {
                assert!(
                    !rect.overlaps(pkg_rect, EPS),
                    "entity {idx} (id={}, rect={:?}) intersects foreign package {pkg_idx} ({:?})",
                    decl.id,
                    rect,
                    pkg_rect
                );
            }
        }
    }

    // Package-vs-package: a package must be contained in its declared
    // parent, and must not overlap a package it is not nested in.
    for (idx, decl) in declared_packages.iter().enumerate() {
        let rect = &package_rects[idx];
        for (other_idx, other_rect) in package_rects.iter().enumerate() {
            if other_idx == idx {
                continue;
            }
            let is_ancestor = {
                let mut cur = decl.parent;
                let mut found = false;
                while let Some(p) = cur {
                    if p == other_idx {
                        found = true;
                        break;
                    }
                    cur = declared_packages[p].parent;
                }
                found
            };
            let is_descendant = {
                let mut cur = declared_packages[other_idx].parent;
                let mut found = false;
                while let Some(p) = cur {
                    if p == idx {
                        found = true;
                        break;
                    }
                    cur = declared_packages[p].parent;
                }
                found
            };
            if is_ancestor {
                assert!(
                    rect.contained_in(other_rect, EPS),
                    "package {idx} ({:?}) is not contained in its declared parent package {other_idx} ({:?})",
                    rect,
                    other_rect
                );
            } else if !is_descendant {
                assert!(
                    !rect.overlaps(other_rect, EPS),
                    "sibling packages {idx} ({:?}) and {other_idx} ({:?}) overlap",
                    rect,
                    other_rect
                );
            }
        }
    }
}
