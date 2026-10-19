//! The PDF Epsilon engine.
//!
//! What it changes, in order:
//! 1. Images placed at more than the target resolution are downsampled to it.
//!    Resolution is measured from where each image is actually drawn on the page.
//! 2. Photographic images are re-encoded as JPEG at the chosen quality.
//!    Flat graphics (screenshots, diagrams, masks) stay lossless.
//! 3. Byte-identical streams are merged, so a logo repeated on every page is stored once.
//! 4. Optional clutter is removed: page thumbnails, editor private data, XMP metadata.
//! 5. Unreferenced objects are dropped, streams are deflated, and objects are packed
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
mod dedup;
mod images;
mod placement;

use std::path::Path;

use lopdf::{Document, ObjectId};
use serde::{Deserialize, Serialize};

pub use analyze::*;
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
    /// Remove embedded page thumbnails. Viewers draw their own.
    pub strip_thumbnails: bool,
    /// Remove `/PieceInfo` editor data (Illustrator, Photoshop). The PDF still
    /// looks the same, but the source application can no longer re-edit it natively.
    pub strip_private_data: bool,
    /// Remove XMP metadata streams. The document info dictionary (title, author) is kept.
    /// This breaks PDF/A conformance.
    pub strip_metadata: bool,
}

impl Options {
    fn base() -> Self {
        Options {
            images: None,
            dedupe: true,
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
    pub duplicates_merged: u32,
    pub objects_removed: u32,
    /// True when the optimised file was not smaller and the original bytes were kept.
    pub kept_original: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("this does not look like a readable PDF ({0})")]
    Parse(String),
    #[error("encrypted PDFs are not supported yet")]
    Encrypted,
    #[error("the optimised file failed verification ({0}); the original was not changed")]
    Verify(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Compress `input` into `output`. The input file is never modified.
pub fn compress_file(input: &Path, output: &Path, options: &Options) -> Result<Report> {
    let bytes = std::fs::read(input)?;
    let (out, report) = compress_bytes(&bytes, options)?;
    std::fs::write(output, out)?;
    Ok(report)
}

/// Compress a PDF held in memory. Returns the new bytes, or the original bytes
/// when nothing could be saved.
pub fn compress_bytes(input: &[u8], options: &Options) -> Result<(Vec<u8>, Report)> {
    let doc = load(input)?;
    let run = optimize(input, doc, options)?;
    Ok((run.bytes, run.report))
}

/// Loads a PDF, refusing encrypted and page-less files.
fn load(input: &[u8]) -> Result<Document> {
    // Checked on the raw bytes first: lopdf decrypts some files on load by itself and
    // fails to decrypt others without saying so, and saving either would lose content
    // or silently drop permission restrictions.
    if has_encrypt_entry(input) {
        return Err(Error::Encrypted);
    }
    let doc = Document::load_mem(input).map_err(|e| Error::Parse(e.to_string()))?;
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
    /// Ids of the original objects that are gone: merged duplicates and pruned objects.
    merged: Vec<ObjectId>,
    pruned: Vec<ObjectId>,
}

fn optimize(input: &[u8], mut doc: Document, options: &Options) -> Result<Run> {
    let mut report = Report { input_bytes: input.len() as u64, ..Report::default() };
    let pages = doc.get_pages().len() as u32;
    report.pages = pages;
    let objects_before = doc.objects.len();

    clean::strip(&mut doc, options);

    let mut changed = Vec::new();
    if let Some(policy) = &options.images {
        let sizes = placement::display_sizes(&doc);
        let fallback = placement::largest_page(&doc);
        let stats = images::recompress(&mut doc, policy, &sizes, fallback);
        report.images_seen = stats.seen;
        report.images_downsampled = stats.downsampled;
        report.images_reencoded = stats.reencoded;
        changed = stats.changed;
    } else {
        report.images_seen = images::count(&doc);
    }

    let merged = if options.dedupe { dedup::merge_identical_streams(&mut doc) } else { Vec::new() };
    report.duplicates_merged = merged.len() as u32;

    let pruned = doc.prune_objects();
    report.objects_removed = objects_before.saturating_sub(doc.objects.len()) as u32;
    doc.renumber_objects();
    doc.compress();

    let out = save(&mut doc)?;

    // Verify: the result must parse and have the same pages.
    let check = Document::load_mem(&out).map_err(|e| Error::Verify(e.to_string()))?;
    let check_pages = check.get_pages().len() as u32;
    if check_pages != pages {
        return Err(Error::Verify(format!("{check_pages} pages instead of {pages}")));
    }

    if out.len() >= input.len() {
        report.output_bytes = input.len() as u64;
        report.kept_original = true;
        return Ok(Run { bytes: input.to_vec(), report, images: changed, merged, pruned });
    }
    report.output_bytes = out.len() as u64;
    Ok(Run { bytes: out, report, images: changed, merged, pruned })
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
    // Object streams and cross-reference streams; lopdf raises the version to 1.5.
    let mut out = Vec::new();
    doc.save_modern(&mut out).map_err(|e| Error::Verify(e.to_string()))?;
    Ok(out)
}
