//! Stage a minimal blockcell tree (`lib.typ` + `src/`) into `$OUT_DIR` so
//! `include_dir!` only embeds the Typst sources we actually load — not the
//! upstream's docs, tests, and examples.
//!
//! `vendor/blockcell` is a git submodule. If it isn't initialized (e.g. the
//! repo was cloned without `--recurse-submodules`), we try to init it once
//! before failing with a clear error.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let src_root = manifest_dir.join("vendor/blockcell");
    let dst_root = out_dir.join("blockcell");

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
    fs::copy(src_root.join("lib.typ"), dst_root.join("lib.typ"))
        .expect("copy blockcell/lib.typ");
    copy_dir(&src_root.join("src"), &dst_root.join("src")).expect("copy blockcell/src");
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

fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
