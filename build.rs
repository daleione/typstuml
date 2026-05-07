//! Stage a minimal blockcell tree into `$OUT_DIR` so `include_dir!` only
//! embeds the Typst sources we actually load — not the upstream's docs,
//! tests, examples, or the Typst sources for blockcell features that
//! TypstUML's codegen never emits.
//!
//! ## What the codegen emits, and what blockcell symbols that touches
//!
//! `src/codegen/record_graph.rs` and `src/codegen/sequence.rs` together
//! emit exactly two blockcell calls:
//!
//! ```text
//!   #record-layout(...)   — JSON / YAML record diagrams
//!                           (vendor/blockcell/src/records.typ)
//!   #seq-puml(...)        — sequence diagrams
//!                           (vendor/blockcell/src/seq-puml.typ)
//! ```
//!
//! `record-layout` only depends on private helpers inside `records.typ`.
//! `seq-puml` pulls in `seq.typ` and `palettes.typ` transitively, and
//! both `seq.typ` and `records.typ` further reach into
//! `internal/metrics.typ`. Every other blockcell symbol is unreached by
//! codegen, so we stage just those files plus a slim `lib.typ` that
//! re-exports the two entry points — saves embedding ~2000 lines of
//! Typst source that would otherwise be parsed on every render.
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
    "palettes.typ",
    "internal/metrics.typ",
];

/// Slim `lib.typ` written to the staged tree. Only re-exports the two
/// public entry points TypstUML's codegen calls. Keep this in sync with
/// `STAGED_SRC_FILES` and the codegen — if a new diagram type starts
/// emitting a different blockcell symbol, add the import here and the
/// owning file (plus its transitive deps) to `STAGED_SRC_FILES`.
const STAGED_LIB_TYP: &str = "\
// Slim re-export for TypstUML. See build.rs for the full rationale.
#import \"src/records.typ\": record-layout
#import \"src/seq-puml.typ\": seq-puml
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
