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
#[cfg(target_arch = "wasm32")]
use std::sync::RwLock;
use std::sync::{Arc, Mutex, OnceLock};

use include_dir::{include_dir, Dir};

use typst::diag::{FileError, FileResult, PackageError};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, Source, VirtualPath};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt};

#[cfg(not(target_arch = "wasm32"))]
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

/// Process-wide font index, shared by every `TypstWorld`. Discovery is the
/// expensive part and only needs to run once per process, so the cache lives
/// behind a shared `Arc`.
///
/// On native targets the fonts come from `typst-kit`'s `FontSearcher` (system
/// fonts plus the embedded defaults), held as lazily-loaded `FontSlot`s. On
/// wasm32 there is no filesystem to search, so we eagerly decode Typst's
/// embedded default fonts from `typst-assets` instead.
#[cfg(not(target_arch = "wasm32"))]
struct FontCache {
    book: LazyHash<FontBook>,
    fonts: Vec<FontSlot>,
}

#[cfg(target_arch = "wasm32")]
struct FontCache {
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

#[cfg(not(target_arch = "wasm32"))]
static FONTS: OnceLock<Arc<FontCache>> = OnceLock::new();

// On wasm we keep the cache behind an `RwLock` so the JS side can append
// extra fonts at runtime via [`add_font`] (e.g. CJK / emoji fetched from a
// CDN on demand). Each render call grabs a fresh snapshot of the `Arc`, so
// fonts added between renders take effect on the next compile.
#[cfg(target_arch = "wasm32")]
static FONTS: OnceLock<RwLock<Arc<FontCache>>> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
fn shared_fonts() -> Arc<FontCache> {
    FONTS
        .get_or_init(|| {
            let f = FontSearcher::new().include_system_fonts(true).search();
            Arc::new(FontCache {
                book: LazyHash::new(f.book),
                fonts: f.fonts,
            })
        })
        .clone()
}

/// Fonts baked into the wasm artifact. The full `typst-assets` font set is
/// 8.4 MB across 17 files (six Libertinus weights, four NCM, three NCM-Math,
/// four DejaVu) — we only need a serif for body text and a mono for code, so
/// we vendor the two regular faces directly under `fonts/`. NCM-Math (3.3 MB)
/// in particular is irrelevant for UML diagrams.
///
/// See `src/runtime/fonts/NOTICE` for upstream licenses (OFL / Bitstream Vera).
/// Typst falls back gracefully when an italic / bold weight is requested,
/// rendering with the regular face — visually fine for diagram labels.
#[cfg(target_arch = "wasm32")]
const EMBEDDED_FONTS: &[&[u8]] = &[
    include_bytes!("fonts/LibertinusSerif-Regular.otf"),
    include_bytes!("fonts/DejaVuSansMono.ttf"),
];

#[cfg(target_arch = "wasm32")]
fn init_wasm_fonts() -> &'static RwLock<Arc<FontCache>> {
    FONTS.get_or_init(|| {
        let mut book = FontBook::new();
        let mut fonts = Vec::new();
        for data in EMBEDDED_FONTS {
            let buffer = Bytes::new(*data);
            for font in Font::iter(buffer) {
                book.push(font.info().clone());
                fonts.push(font);
            }
        }
        RwLock::new(Arc::new(FontCache {
            book: LazyHash::new(book),
            fonts,
        }))
    })
}

#[cfg(target_arch = "wasm32")]
fn shared_fonts() -> Arc<FontCache> {
    init_wasm_fonts()
        .read()
        .expect("font cache poisoned")
        .clone()
}

/// Append a font file's faces to the wasm font cache. Returns the number of
/// faces extracted (a TTC may contain several). Subsequent renders pick up
/// the new fonts automatically — Typst's fallback selection will use them
/// when the embedded defaults don't cover a requested glyph.
///
/// Used by the JS playground to fetch CJK / emoji fonts on demand. Each call
/// rebuilds `FontBook` + `Vec<Font>` and atomically swaps the shared `Arc`,
/// so in-flight renders holding the old snapshot finish on the old set.
#[cfg(target_arch = "wasm32")]
pub fn add_font(data: Vec<u8>) -> Result<usize, String> {
    let buffer = Bytes::new(data);
    let new_fonts: Vec<Font> = Font::iter(buffer).collect();
    if new_fonts.is_empty() {
        return Err("no usable font faces in file".to_string());
    }
    let count = new_fonts.len();

    let lock = init_wasm_fonts();
    let mut guard = lock.write().map_err(|_| "font cache poisoned".to_string())?;

    // LazyHash captures a hash on construction, so a fresh LazyHash::new
    // makes Typst's font index re-scan the new entries on the next compile.
    let mut book: FontBook = (*guard.book).clone();
    let mut fonts = guard.fonts.clone();
    for f in &new_fonts {
        book.push(f.info().clone());
    }
    fonts.extend(new_fonts);

    *guard = Arc::new(FontCache {
        book: LazyHash::new(book),
        fonts,
    });
    Ok(count)
}

pub struct TypstWorld {
    root: PathBuf,
    main: Source,
    library: LazyHash<Library>,
    fonts: Arc<FontCache>,
    files: Mutex<HashMap<FileId, FileEntry>>,
    // wasm32 has no wall clock available to `time`; `today()` returns `None`
    // there instead (no diagram type depends on the document date).
    #[cfg(not(target_arch = "wasm32"))]
    time: time::OffsetDateTime,
}

impl TypstWorld {
    pub fn new(root: PathBuf, source: String) -> Self {
        Self {
            root,
            main: Source::detached(source),
            library: LazyHash::new(Library::default()),
            fonts: shared_fonts(),
            files: Mutex::new(HashMap::new()),
            #[cfg(not(target_arch = "wasm32"))]
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
        &self.fonts.book
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

    #[cfg(not(target_arch = "wasm32"))]
    fn font(&self, id: usize) -> Option<Font> {
        self.fonts.fonts.get(id)?.get()
    }

    #[cfg(target_arch = "wasm32")]
    fn font(&self, id: usize) -> Option<Font> {
        self.fonts.fonts.get(id).cloned()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn today(&self, offset: Option<i64>) -> Option<Datetime> {
        let offset = offset.unwrap_or(0);
        let offset = time::UtcOffset::from_hms(offset.try_into().ok()?, 0, 0).ok()?;
        let time = self.time.checked_to_offset(offset)?;
        Some(Datetime::Date(time.date()))
    }

    #[cfg(target_arch = "wasm32")]
    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}
