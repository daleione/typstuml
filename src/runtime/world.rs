//! `typst::World` implementation for TypstUML.
//!
//! Adapted from `typst-as-library`. Differences:
//!   - blockcell's Typst sources are embedded at compile time and served
//!     from a virtual root (`/blockcell/...`).
//!   - Typst package downloads are disabled (returns NotFound). The CLI is
//!     fully offline by design.
//!
//! `typst::World` requires `Sync`, so the file cache uses `Mutex` even
//! though Typst's compile loop is single-threaded.

use std::collections::HashMap;
use std::path::{Component, PathBuf};
use std::sync::Mutex;

use include_dir::{include_dir, Dir};

use typst::diag::{FileError, FileResult, PackageError};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::Library;
use typst_kit::fonts::{FontSearcher, FontSlot};

/// Vendored `blockcell` sources, baked into the binary at compile time.
/// `build.rs` stages a minimal tree (`lib.typ` + `src/`) from
/// `vendor/blockcell/` (a git subtree of daleione/blockcell) into
/// `$OUT_DIR`; the directory contents are mounted at `/blockcell/`.
static BLOCKCELL: Dir<'_> = include_dir!("$OUT_DIR/blockcell");

#[derive(Clone, Debug)]
struct FileEntry {
    bytes: Bytes,
    source: Option<Source>,
}

impl FileEntry {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: Bytes::new(bytes),
            source: None,
        }
    }

    fn source(&mut self, id: FileId) -> FileResult<Source> {
        if let Some(source) = &self.source {
            return Ok(source.clone());
        }
        let contents = std::str::from_utf8(&self.bytes).map_err(|_| FileError::InvalidUtf8)?;
        let trimmed = contents.trim_start_matches('\u{feff}');
        let source = Source::new(id, trimmed.into());
        self.source = Some(source.clone());
        Ok(source)
    }
}

pub struct TypstWorld {
    root: PathBuf,
    main: Source,
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<FontSlot>,
    files: Mutex<HashMap<FileId, FileEntry>>,
    time: time::OffsetDateTime,
}

impl TypstWorld {
    pub fn new(root: PathBuf, source: String) -> Self {
        let fonts = FontSearcher::new().include_system_fonts(true).search();
        Self {
            root,
            main: Source::detached(source),
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(fonts.book),
            fonts: fonts.fonts,
            files: Mutex::new(HashMap::new()),
            time: time::OffsetDateTime::now_utc(),
        }
    }

    fn lookup(&self, id: FileId) -> FileResult<FileEntry> {
        let mut files = self.files.lock().map_err(|_| FileError::AccessDenied)?;
        if let Some(entry) = files.get(&id) {
            return Ok(entry.clone());
        }

        if let Some(pkg) = id.package() {
            return Err(FileError::Package(PackageError::NotFound(pkg.clone())));
        }

        let bytes = if let Some(content) = read_embedded(id.vpath()) {
            content
        } else {
            let path = id
                .vpath()
                .resolve(&self.root)
                .ok_or(FileError::AccessDenied)?;
            std::fs::read(&path).map_err(|e| FileError::from_io(e, &path))?
        };

        let entry = FileEntry::new(bytes);
        files.insert(id, entry.clone());
        Ok(entry)
    }
}

/// Resolve a `VirtualPath` against the embedded `blockcell` directory.
///
/// Typst gives us a rooted virtual path (`/blockcell/lib.typ`); we walk
/// its components, require the first to be `blockcell`, and look the
/// remainder up inside the embedded `Dir` (which uses forward slashes
/// regardless of host OS).
fn read_embedded(vpath: &VirtualPath) -> Option<Vec<u8>> {
    let mut comps = vpath.as_rooted_path().components();
    if !matches!(comps.next(), Some(Component::RootDir)) {
        return None;
    }
    let first = comps.next()?;
    if first.as_os_str() != "blockcell" {
        return None;
    }
    let rel = comps
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(str::to_string),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    BLOCKCELL.get_file(&rel).map(|f| f.contents().to_vec())
}

impl typst::World for TypstWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }
        let mut entry = self.lookup(id)?;
        let source = entry.source(id)?;
        // Persist the parsed source on the cache entry so subsequent reads
        // skip UTF-8 validation.
        if let Ok(mut files) = self.files.lock() {
            files.insert(id, entry);
        }
        Ok(source)
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.lookup(id).map(|f| f.bytes.clone())
    }

    fn font(&self, id: usize) -> Option<Font> {
        self.fonts[id].get()
    }

    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        let offset = offset.unwrap_or(0);
        let offset = time::UtcOffset::from_hms(offset.try_into().ok()?, 0, 0).ok()?;
        let time = self.time.checked_to_offset(offset)?;
        Some(Datetime::Date(time.date()))
    }
}
