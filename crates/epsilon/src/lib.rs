//! The PDF Epsilon engine.
//!
//! What it changes, in order:
//! 1. Images placed at more than the target resolution are downsampled to it.
//!    Resolution is measured from where each image is actually drawn on the page.
//! 2. Photographic images are re-encoded as JPEG at the chosen quality.
//!    Flat graphics (screenshots, diagrams, masks) stay lossless.
//! 3. Fully embedded TrueType CID fonts are subset to the glyphs drawn, and embedded fonts
//!    lose tables no viewer reads, such as screen bitmaps. Outlines in use stay exactly.
//! 4. Byte-identical streams are merged, so a logo repeated on every page is stored once.
//! 5. Optional clutter is removed: page thumbnails, editor private data, XMP metadata.
//! 6. Unreferenced objects are dropped, streams are deflated, and objects are packed
//!    into compressed object streams.
//!
//! What it keeps: text, fonts, vector drawing, links, bookmarks, forms, annotations,
//! attachments and page structure. The result is reloaded and its page count checked
//! before it is accepted, and if it is not smaller the original bytes are returned.
//!
//! [`analyze_bytes`] explains a file before anything is changed: where its bytes go,
//! page by page, and what each preset saves, measured by running it in memory.

mod analyze;
mod clean;
mod message;
mod dedup;
mod fonts;
mod images;
mod limits;
mod memo;
mod placement;
mod revisions;
mod subset;
mod system;
mod verify;

use std::path::Path;

use lopdf::{Document, ObjectId};
use serde::{Deserialize, Serialize};

pub use analyze::*;
pub use message::{Arg, Message};
/// Standard base64 with padding, for handing images to the interface as `data:` URLs.
pub fn to_base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            out.push(if i <= c.len() { T[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    out
}
pub use images::ImagePolicy;

/// Ready-made settings, from untouched pixels to smallest file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    /// No pixel is changed. Structure, streams and clutter only.
    Lossless,
    /// 300 dpi, JPEG quality 85. For documents that will be printed.
    Print,
    /// 150 dpi, JPEG quality 75. Reads well on any screen.
    Balanced,
    /// 96 dpi, JPEG quality 60. For email and upload limits.
    Small,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Lossless, Preset::Print, Preset::Balanced, Preset::Small];

    pub fn options(self) -> Options {
        let image = |max_dpi: f32, jpeg_quality: u8| Some(ImagePolicy { max_dpi, jpeg_quality });
        match self {
            Preset::Lossless => Options { images: None, ..Options::base() },
            Preset::Print => Options { images: image(300.0, 85), ..Options::base() },
            Preset::Balanced => Options {
                images: image(150.0, 75),
                strip_private_data: true,
                ..Options::base()
            },
            Preset::Small => Options {
                images: image(96.0, 60),
                strip_private_data: true,
                strip_metadata: true,
                ..Options::base()
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// `None` leaves every image's pixels untouched.
    pub images: Option<ImagePolicy>,
    /// Merge byte-identical streams.
    pub dedupe: bool,
    /// Drop font tables no PDF viewer reads, such as embedded screen bitmaps. Lossless.
    /// Settings saved before this option existed turn it on.
    #[serde(default = "yes")]
    pub strip_font_tables: bool,
    /// Keep only the glyphs the document draws in fonts embedded in full. The pages look the
    /// same and text stays searchable, but the fonts can no longer type new text.
    #[serde(default = "yes")]
    pub subset_fonts: bool,
    /// Remove embedded page thumbnails. Viewers draw their own.
    pub strip_thumbnails: bool,
    /// Remove `/PieceInfo` editor data (Illustrator, Photoshop). The PDF still
    /// looks the same, but the source application can no longer re-edit it natively.
    pub strip_private_data: bool,
    /// Remove XMP metadata streams. The document info dictionary (title, author) is kept.
    /// This breaks PDF/A conformance.
    pub strip_metadata: bool,
}

fn yes() -> bool {
    true
}

impl Options {
    fn base() -> Self {
        Options {
            images: None,
            dedupe: true,
            strip_font_tables: true,
            subset_fonts: true,
            strip_thumbnails: true,
            strip_private_data: false,
            strip_metadata: false,
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Preset::Balanced.options()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub pages: u32,
    pub images_seen: u32,
    pub images_downsampled: u32,
    pub images_reencoded: u32,
    pub fonts_subset: u32,
    pub fonts_slimmed: u32,
    pub duplicates_merged: u32,
    pub objects_removed: u32,
    /// True when the optimised file was not smaller and the original bytes were kept.
    pub kept_original: bool,
    /// True when subsetting was undone because the check found a glyph in use removed.
    /// See `verify.rs`.
    #[serde(default)]
    pub fonts_restored: bool,
}

/// Stops work in progress: a progress callback raises it as a panic payload
/// (`std::panic::panic_any(Cancelled)`), the engine passes it on, and the caller catches it
/// with `std::panic::catch_unwind`. Work is only ever stopped between steps, and nothing is
/// written, so stopping loses nothing.
pub struct Cancelled;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("this does not look like a readable PDF ({0})")]
    Parse(String),
    #[error("encrypted PDFs are not supported yet")]
    Encrypted,
    #[error("the file is larger than {} GB, the most PDF Epsilon reads", limits::FILE >> 30)]
    TooLarge,
    #[error("the file has more than {} objects, more than PDF Epsilon handles safely", limits::OBJECTS)]
    TooComplex,
    #[error("the optimised file failed verification ({0}); the original was not changed")]
    Verify(String),
    #[error("{0} parts of the file could not be read, so it is left unchanged")]
    Damaged(usize),
    #[error("something went wrong inside PDF Epsilon ({0})")]
    Internal(String),
}

impl Error {
    /// A stable code for the interface, which shows its own translated text for it.
    /// See `error.*` in `ui/locales`.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Io(_) => "io",
            Error::Parse(_) => "parse",
            Error::Encrypted => "encrypted",
            Error::TooLarge => "tooLarge",
            Error::TooComplex => "tooComplex",
            Error::Verify(_) => "verify",
            Error::Damaged(_) => "damaged",
            Error::Internal(_) => "internal",
        }
    }

    /// The technical detail behind the code, untranslated. Empty when there is none.
    pub fn detail(&self) -> String {
        match self {
            Error::Io(e) => e.to_string(),
            Error::Parse(d) | Error::Verify(d) | Error::Internal(d) => d.clone(),
            Error::Damaged(n) => format!("{n} unreadable objects"),
            Error::Encrypted | Error::TooLarge | Error::TooComplex => String::new(),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Compress `input` into `output`. The input file is never modified.
pub fn compress_file(input: &Path, output: &Path, options: &Options) -> Result<Report> {
    let bytes = read_input(input)?;
    let (out, report) = compress_bytes(&bytes, options)?;
    std::fs::write(output, out)?;
    Ok(report)
}

/// Compress a PDF held in memory. Returns the new bytes, or the original bytes
/// when nothing could be saved.
pub fn compress_bytes(input: &[u8], options: &Options) -> Result<(Vec<u8>, Report)> {
    compress_bytes_with_progress(input, options, &|_| {})
}

/// Like [`compress_bytes`], reporting progress from 0 to 1 as it goes.
pub fn compress_bytes_with_progress(input: &[u8], options: &Options, progress: &dyn Fn(f32)) -> Result<(Vec<u8>, Report)> {
    let doc = load(input)?;
    let run = optimize(input, doc, options, None, progress)?;
    progress(1.0);
    Ok((run.bytes, run.report))
}

/// Reads a file, refusing one too large to hold in memory safely.
pub fn read_input(path: &Path) -> Result<Vec<u8>> {
    if std::fs::metadata(path)?.len() > limits::FILE {
        return Err(Error::TooLarge);
    }
    Ok(std::fs::read(path)?)
}

/// Loads a PDF, refusing encrypted and page-less files.
fn load(input: &[u8]) -> Result<Document> {
    // Checked on the raw bytes first: lopdf decrypts some files on load by itself and
    // fails to decrypt others without saying so, and saving either would lose content
    // or silently drop permission restrictions.
    if has_encrypt_entry(input) {
        return Err(Error::Encrypted);
    }
    // Hostile files: predictor rows sized to exhaust memory, and object or xref streams that
    // inflate without end, are both stopped before or during loading. See limits.rs.
    if !limits::raw_params_ok(input) {
        return Err(Error::Parse("implausible stream parameters".into()));
    }
    if limits::raw_object_estimate(input) > limits::OBJECTS {
        return Err(Error::TooComplex);
    }
    let doc = load_bounded(input).map_err(|e| Error::Parse(e.to_string()))?;
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(Error::Encrypted);
    }
    if doc.get_pages().is_empty() {
        return Err(Error::Parse("no pages found".into()));
    }
    Ok(doc)
}

/// What one run of the pipeline did, object by object, for the analyzer.
struct Run {
    bytes: Vec<u8>,
    report: Report,
    images: Vec<images::Change>,
    fonts: Vec<fonts::Change>,
    subsets: Vec<fonts::Change>,
    /// Ids of the original objects that are gone: merged duplicates and pruned objects.
    merged: Vec<ObjectId>,
    pruned: Vec<ObjectId>,
}

/// Runs the pipeline. `progress` gets the share done, from 0 to 1; images take most of it.
/// `share` is what the presets of one analysis share (see `images::Share`); `None` for a save.
fn optimize(input: &[u8], mut doc: Document, options: &Options, share: Option<&images::Share>, progress: &dyn Fn(f32)) -> Result<Run> {
    // lopdf reads leniently; a copy written from what it could not read correctly would change
    // the document without saying so.
    let lost = damage(input, &doc);
    if lost > 0 {
        return Err(Error::Damaged(lost));
    }
    let mut report = Report { input_bytes: input.len() as u64, ..Report::default() };
    // Counted before anything changes, and compared with the saved copy below.
    let counts = verify::counts(&doc);
    let pages = doc.get_pages().len() as u32;
    report.pages = pages;
    let objects_before = doc.objects.len();

    clean::strip(&mut doc, options);

    let mut changed = Vec::new();
    if let Some(policy) = &options.images {
        let mut sizes = placement::display_sizes(&doc);
        let fallback = placement::largest_page(&doc);
        // An image also shown in an annotation, pattern or soft mask may be drawn larger there.
        for id in placement::images_outside_content(&doc) {
            let s = sizes.entry(id).or_insert(fallback);
            *s = (s.0.max(fallback.0), s.1.max(fallback.1));
        }
        let stats = images::recompress(&mut doc, policy, &sizes, fallback, share, &|f| progress(0.05 + 0.75 * f));
        report.images_seen = stats.seen;
        report.images_downsampled = stats.downsampled;
        report.images_reencoded = stats.reencoded;
        changed = stats.changed;
    } else {
        report.images_seen = images::count(&doc);
    }

    progress(0.8);
    let subsets = if options.subset_fonts {
        let (changes, undo) = subset::subset_fonts(&mut doc);
        // Every glyph a page draws must still be there; if not, the fonts stay whole.
        match verify::glyphs_kept(&doc, &undo.programs) {
            Ok(()) => changes,
            Err(_) => {
                undo.restore(&mut doc);
                report.fonts_restored = true;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    report.fonts_subset = subsets.len() as u32;
    let slimmed = if options.strip_font_tables { fonts::strip_tables(&mut doc) } else { Vec::new() };
    report.fonts_slimmed = slimmed.len() as u32;

    let merged = if options.dedupe { dedup::merge_identical_streams(&mut doc) } else { Vec::new() };
    report.duplicates_merged = merged.len() as u32;

    let pruned = prune(&mut doc);
    report.objects_removed = objects_before.saturating_sub(doc.objects.len()) as u32;
    renumber(&mut doc);
    doc.compress();

    progress(0.9);
    let out = save(&mut doc)?;
    progress(0.97);

    // Verify: the result must parse and have the same pages, annotations, resources,
    // bookmarks, form fields, attachments and destinations. See verify.rs.
    let check = load_bounded(&out).map_err(|e| Error::Verify(e.to_string()))?;
    let check_pages = check.get_pages().len() as u32;
    if check_pages != pages {
        return Err(Error::Verify(format!("{check_pages} pages instead of {pages}")));
    }
    verify::compare(&counts, &verify::counts(&check)).map_err(Error::Verify)?;

    if out.len() >= input.len() {
        report.output_bytes = input.len() as u64;
        report.kept_original = true;
        return Ok(Run { bytes: input.to_vec(), report, images: changed, fonts: slimmed, subsets, merged, pruned });
    }
    report.output_bytes = out.len() as u64;
    Ok(Run { bytes: out, report, images: changed, fonts: slimmed, subsets, merged, pruned })
}

/// Removes every object nothing refers to and returns their ids. lopdf's own
/// `prune_objects` checks visited objects against a list, which is quadratic: half a million
/// objects took over 100 seconds. This uses a hash set.
fn prune(doc: &mut Document) -> Vec<ObjectId> {
    let live = analyze::reachable(doc);
    let dead: Vec<ObjectId> = doc.objects.keys().filter(|id| !live.contains(id)).copied().collect();
    for id in &dead {
        doc.objects.remove(id);
    }
    dead
}

/// Numbers objects 1, 2, 3, ... in their current order and rewrites every reference. lopdf's
/// `renumber_objects` is quadratic like its pruning. A reference to an object that does not
/// exist becomes null, which is what the PDF specification says it means; left as it was,
/// it could end up pointing at a different object.
fn renumber(doc: &mut Document) {
    let map: std::collections::HashMap<ObjectId, ObjectId> =
        doc.objects.keys().enumerate().map(|(i, id)| (*id, (i as u32 + 1, 0))).collect();
    fn rewrite(obj: &mut lopdf::Object, map: &std::collections::HashMap<ObjectId, ObjectId>) {
        use lopdf::Object;
        match obj {
            Object::Reference(id) => match map.get(id) {
                Some(new) => *id = *new,
                None => *obj = Object::Null,
            },
            Object::Array(items) => items.iter_mut().for_each(|o| rewrite(o, map)),
            Object::Dictionary(d) => d.iter_mut().for_each(|(_, o)| rewrite(o, map)),
            Object::Stream(s) => s.dict.iter_mut().for_each(|(_, o)| rewrite(o, map)),
            _ => {}
        }
    }
    let old = std::mem::take(&mut doc.objects);
    for (id, mut obj) in old {
        rewrite(&mut obj, &map);
        doc.objects.insert(map[&id], obj);
    }
    for (_, v) in doc.trailer.iter_mut() {
        rewrite(v, &map);
    }
    doc.max_id = map.len() as u32;
}

/// How many parts of the document lopdf did not read faithfully. Each one is a reason to leave
/// the file unchanged:
/// - an object the file refers to and lists in its cross-reference table, but that lopdf left
///   out: it skips an object it cannot parse, and a whole object stream over its size limit;
/// - an object a later revision deleted, which lopdf kept from an older revision
///   (see `revisions.rs`), while something still refers to it;
/// - a stream whose data lopdf could not locate, for a missing or broken `/Length`.
///
/// References to objects the file never listed are allowed by the format (they mean null) and
/// are not counted. Walks with its own stack: nesting in a hostile file is unbounded.
fn damage(input: &[u8], doc: &Document) -> usize {
    use lopdf::{xref::XrefEntry, Object};
    let listed = |id: &ObjectId| match doc.reference_table.entries.get(&id.0) {
        Some(XrefEntry::Normal { generation, .. }) => u32::from(*generation) == u32::from(id.1),
        Some(XrefEntry::Compressed { .. }) => id.1 == 0,
        _ => false,
    };
    let deleted = revisions::deleted_objects(input);
    let mut bad = std::collections::BTreeSet::new();
    for (id, obj) in &doc.objects {
        if let Object::Stream(s) = obj {
            if s.start_position.is_some() && s.content.is_empty() {
                let length = s.dict.get(b"Length").ok().and_then(|v| doc.dereference(v).ok()).and_then(|(_, v)| v.as_float().ok());
                if length != Some(0.0) {
                    bad.insert(*id);
                }
            }
        }
    }
    let mut stack: Vec<&Object> = doc.objects.values().collect();
    stack.extend(doc.trailer.iter().map(|(_, v)| v));
    while let Some(obj) = stack.pop() {
        match obj {
            Object::Reference(id) => {
                let present = doc.objects.contains_key(id);
                if (!present && listed(id)) || (present && deleted.contains(&id.0)) {
                    bad.insert(*id);
                }
            }
            Object::Array(items) => stack.extend(items.iter()),
            Object::Dictionary(d) => stack.extend(d.iter().map(|(_, v)| v)),
            Object::Stream(s) => stack.extend(s.dict.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
    bad.len()
}

fn load_bounded(bytes: &[u8]) -> lopdf::Result<Document> {
    let mut options = lopdf::LoadOptions::default();
    options.max_decompressed_size = Some(limits::STREAM);
    Document::load_mem_with_options(bytes, options)
}

/// True when the file has an `/Encrypt` key followed by a delimiter, so `/EncryptMetadata`
/// does not count. The key sits in the trailer, or in a cross-reference stream's
/// dictionary, and neither is ever compressed.
fn has_encrypt_entry(bytes: &[u8]) -> bool {
    const KEY: &[u8] = b"/Encrypt";
    bytes.windows(KEY.len() + 1).any(|w| {
        w.starts_with(KEY) && matches!(w[KEY.len()], b' ' | b'\r' | b'\n' | b'\t' | b'\x0c' | b'\0' | b'<' | b'/' | b'[')
    })
}

fn save(doc: &mut Document) -> Result<Vec<u8>> {
    // The copy is one complete revision. Pointers to the original's earlier cross-reference
    // sections would point into nothing (lopdf keeps them from the input's trailer when it
    // could not follow them), and a reader then fails to open the copy.
    doc.trailer.remove(b"Prev");
    doc.trailer.remove(b"XRefStm");
    // Object streams and cross-reference streams; lopdf raises the version to 1.5.
    let mut out = Vec::new();
    doc.save_modern(&mut out).map_err(|e| Error::Verify(e.to_string()))?;
    Ok(out)
}
