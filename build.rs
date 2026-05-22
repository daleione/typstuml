//! Stage a minimal blockcell tree into `$OUT_DIR` so `include_dir!` only
//! embeds the Typst sources we actually load — not the upstream's docs,
//! tests, examples, or the Typst sources for blockcell features that
//! TypstUML's codegen never emits.
//!
//! ## What the codegen emits, and what blockcell symbols that touches
//!
//! `src/codegen/record_graph.rs`, `src/codegen/sequence.rs`,
//! `src/codegen/wbs.rs`, and `src/codegen/class.rs` together emit these
//! blockcell calls:
//!
//! ```text
//!   #record-layout(...)   — JSON / YAML record diagrams
//!                           (vendor/blockcell/src/records.typ)
//!   #seq-puml(...)        — sequence diagrams
//!                           (vendor/blockcell/src/seq-puml.typ)
//!   #tree(...) / #node[…] — WBS diagrams
//!   #mindmap(...)         — mind-map diagrams
//!                           (vendor/blockcell/src/tree.typ)
//!   #cuca-layout(...)     — cuca diagrams (class / component /
//!                           deployment / use case)
//!                           (vendor/blockcell/src/cuca.typ)
//! ```
//!
//! `record-layout` only depends on private helpers inside `records.typ`.
//! `seq-puml` pulls in `seq.typ` and `palettes.typ` transitively, and
//! both `seq.typ` and `records.typ` further reach into
//! `internal/metrics.typ`. `tree.typ` only needs `palettes.typ`. Every
//! other blockcell symbol is unreached by codegen, so we stage just those
//! files plus a slim `lib.typ` that re-exports the entry points — saves
//! embedding the rest of the upstream library that would otherwise be
//! parsed on every render.
//!
//! Activity diagrams add:
//!
//! ```text
//!   #flow-col(...)        — vertical step composition
//!   #branch-merge(...)    — if-else with rejoining branches
//!   #switch(...)          — N-way diamond fan-out
//!   #fork-bar(...)        — concurrent fork / split with sync-bars
//!   #flow-loop(...)       — while / repeat back-edge
//!   #process / #decision  — action / decision atoms
//!   #start-marker / #stop-marker / #end-marker / #detach-marker
//! ```
//!
//! These live in `flows.typ` + `composites.typ::flow-col` +
//! `atoms.typ`, transitively pulling in `containers.typ` and
//! `internal/stroke.typ`.
//!
//! State diagrams add:
//!
//! ```text
//!   #state-layout(...)   — UML state machines
//!                          (vendor/blockcell/src/states.typ)
//! ```
//!
//! `states.typ` only depends on `palettes.typ`.
//!
//! `vendor/blockcell` itself is a git submodule and stays unchanged —
//! the slimming happens only inside `$OUT_DIR/blockcell`.

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Files inside `vendor/blockcell/src/` reachable from the two staged
/// entry points. Update together with `STAGED_LIB_TYP` and the codegen.
const STAGED_SRC_FILES: &[&str] = &[
    "records.typ",
    "seq-puml.typ",
    "seq.typ",
    "tree.typ",
    "cuca.typ",
    "cuca/theme.typ",
    "cuca/shape-card.typ",
    "cuca/shape-desc.typ",
    "cuca/edges.typ",
    "states.typ",
    "atoms.typ",
    "composites.typ",
    "containers.typ",
    "flows.typ",
    "palettes.typ",
    "internal/metrics.typ",
    "internal/stroke.typ",
];

/// Slim `lib.typ` written to the staged tree. Only re-exports the two
/// public entry points TypstUML's codegen calls. Keep this in sync with
/// `STAGED_SRC_FILES` and the codegen — if a new diagram type starts
/// emitting a different blockcell symbol, add the import here and the
/// owning file (plus its transitive deps) to `STAGED_SRC_FILES`.
const STAGED_LIB_TYP: &str = "\
// Slim re-export for TypstUML. See build.rs for the full rationale.
#import \"src/records.typ\": record-layout, record-probe
#import \"src/seq-puml.typ\": seq-puml
#import \"src/tree.typ\": tree, node, mindmap
#import \"src/cuca.typ\": cuca-layout, cuca-probe, container-probe
#import \"src/states.typ\": state-layout, state-probe, state-note-probe, state-edge-label-probe
#import \"src/atoms.typ\": process, decision, terminal, junction, edge, flow-node
#import \"src/composites.typ\": flow-col, section
#import \"src/flows.typ\": branch, branch-merge, switch, case, n-way, fork-bar, flow-loop, start-marker, stop-marker, end-marker, detach-marker, partition, flow-note, with-notes, swimlane, lane, swimlane-layout, swimlane-probe
";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src_root = manifest_dir.join("vendor/blockcell");
    let dst_root = out_dir.join("blockcell");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=vendor/blockcell/lib.typ");
    println!("cargo:rerun-if-changed=vendor/blockcell/src");
    println!("cargo:rerun-if-changed=.gitmodules");

    if !src_root.join("lib.typ").exists() {
        try_init_submodule(&manifest_dir);
    }
    if !src_root.join("lib.typ").exists() {
        panic!(
            "vendor/blockcell is empty. Run `git submodule update --init vendor/blockcell`, \
             or clone with `git clone --recurse-submodules ...`."
        );
    }

    if dst_root.exists() {
        fs::remove_dir_all(&dst_root).expect("clean staged blockcell");
    }
    fs::create_dir_all(&dst_root).expect("create staged blockcell");
    fs::write(dst_root.join("lib.typ"), STAGED_LIB_TYP).expect("write staged lib.typ");

    for rel in STAGED_SRC_FILES {
        let src = src_root.join("src").join(rel);
        let dst = dst_root.join("src").join(rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).expect("create staged subdir");
        }
        fs::copy(&src, &dst)
            .unwrap_or_else(|e| panic!("copy {} → {}: {e}", src.display(), dst.display()));
    }
}

fn try_init_submodule(repo_root: &Path) {
    // Source tarballs (e.g. from crates.io) ship without `.git`; files should
    // already be present, so there's nothing to init.
    if !repo_root.join(".git").exists() {
        return;
    }
    let _ = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args([
            "submodule",
            "update",
            "--init",
            "--depth",
            "1",
            "vendor/blockcell",
        ])
        .status();
}
