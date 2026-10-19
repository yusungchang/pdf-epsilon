//! Explain a PDF before changing it.
//!
//! 1. Every object's size on disk is measured from the cross-reference offsets. Objects
//!    packed into object streams share their container's size in proportion to their
//!    length. Bytes outside every current object are file overhead and earlier revisions.
//! 2. Each object is put in exactly one category, so the categories add up to the file
//!    size and nothing is counted twice.
//! 3. Every page's content is walked to find what it draws: which images, at what
//!    resolution and how much of each is inside the visible page, and which fonts.
//! 4. Every preset is run in memory and its output measured. Savings come from those
//!    runs, not from predictions. Nothing is written to disk.
//! 5. Findings for the whole file, and issues for each page, are drawn from all of that.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::time::Instant;

use lopdf::xref::XrefEntry;
use lopdf::{Dictionary, Document, Object, ObjectId};
use serde::Serialize;

use crate::message::{msg, Bytes, Message, Term};
use crate::placement::{self, Draw};
use crate::{images, Error, Preset, Result, Run};

/// Where a byte of the file belongs. Every object is in exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    /// Image XObjects and their soft masks.
    Images,
    /// Font dictionaries, descriptors, embedded font programs, encodings and ToUnicode maps.
    Fonts,
    /// Page content streams: the drawing instructions for text and vector art.
    PageContent,
    /// Form XObjects, patterns and shadings: reusable or complex vector drawing.
    VectorGraphics,
    /// Links, comments, form fields and their appearance streams.
    Annotations,
    /// Embedded files.
    Attachments,
    /// Embedded page thumbnails.
    Thumbnails,
    /// `/PieceInfo` data that lets Illustrator or Photoshop re-edit the file.
    EditorData,
    /// XMP metadata streams and the document information dictionary.
    Metadata,
    /// The structure tree used by screen readers and reflow.
    Tagging,
    /// Bookmarks and named destinations.
    Navigation,
    /// ICC colour profiles, including PDF/A output intents.
    ColourProfiles,
    /// The page tree, catalog, resource dictionaries and everything else in use.
    Structure,
    /// Objects that nothing in the current document refers to.
    Unused,
    /// Cross-reference data, earlier revisions left by incremental saves, and padding.
    Overhead,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Analysis {
    pub file: FileInfo,
    /// Largest first. Adds up to the file size.
    pub categories: Vec<CategoryUsage>,
    /// One per preset, in [`Preset::ALL`] order. Measured by running each in memory.
    pub presets: Vec<PresetResult>,
    /// Warnings first, then the rest by bytes involved, largest first.
    pub findings: Vec<Finding>,
    pub pages: Vec<PageInfo>,
    /// Largest first.
    pub images: Vec<ImageInfo>,
    /// Largest first.
    pub fonts: Vec<FontInfo>,
    pub attachments: Vec<Attachment>,
    /// Time taken by the analysis, including the preset runs.
    pub millis: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub bytes: u64,
    pub pages: u32,
    pub version: String,
    pub producer: Option<String>,
    pub creator: Option<String>,
    pub objects: u32,
    /// 1 for a file written in one go; more when it was saved incrementally.
    pub revisions: u32,
    pub linearized: bool,
    pub object_streams: bool,
    pub tagged: bool,
    pub signed: bool,
    /// For example `"2B"` when the XMP metadata claims PDF/A-2b.
    pub pdfa: Option<String>,
    pub hidden_layers: bool,
    /// Pages whose content could not be fully decoded. Their numbers are less precise.
    pub unreadable_pages: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryUsage {
    pub category: Category,
    pub bytes: u64,
    pub objects: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetResult {
    pub preset: Preset,
    /// Size of the file this preset produces. Equal to the input when it cannot save anything.
    pub output_bytes: u64,
    pub kept_original: bool,
    /// Set when this preset failed, for example because verification failed.
    pub error: Option<String>,
    /// Where the saving came from. Adds up to the total saving.
    pub savings: Vec<Saving>,
    pub images_changed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SavingKind {
    Images,
    Duplicates,
    Thumbnails,
    EditorData,
    Metadata,
    Unused,
    /// Repacking: object streams, compressed streams, dropped revisions and padding.
    /// Whatever is left of the measured saving once the others are taken out.
    Rewrite,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saving {
    pub kind: SavingKind,
    /// Can be negative for `Rewrite`, when repacking costs a little.
    pub bytes: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    High,
    Medium,
    Low,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// At least one preset removes or reduces it. See `savings`.
    Fixed,
    /// Recognised, but PDF Epsilon cannot reduce it yet.
    NotYet,
    /// Deliberately kept, because removing it would lose something.
    Kept,
    /// Something to know before compressing.
    Warning,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub kind: &'static str,
    pub severity: Severity,
    pub status: Status,
    pub title: Message,
    /// Sentences, shown one after another.
    pub detail: Vec<Message>,
    /// Bytes of the file this finding is about.
    pub bytes: u64,
    /// Measured saving for each preset, in [`Preset::ALL`] order.
    pub savings: Vec<u64>,
    pub pages: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub number: u32,
    /// Visible size in points.
    pub width: f32,
    pub height: f32,
    /// This page's share of the file: its own objects, plus shared ones divided by
    /// the number of pages that use them.
    pub bytes: u64,
    pub content: u64,
    pub images: u64,
    pub fonts: u64,
    pub graphics: u64,
    /// Annotations, thumbnail and editor data.
    pub other: u64,
    /// Object numbers, matching [`ImageInfo::obj`].
    pub image_objs: Vec<u32>,
    /// Object numbers, matching [`FontInfo::obj`].
    pub font_objs: Vec<u32>,
    pub issues: Vec<PageIssue>,
    /// Measured saving on this page for each preset, in [`Preset::ALL`] order.
    pub savings: Vec<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageIssue {
    pub kind: &'static str,
    pub status: Status,
    pub message: Message,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageInfo {
    pub obj: u32,
    pub width: u32,
    pub height: u32,
    pub bits: u32,
    pub colour: String,
    pub encoding: String,
    pub bytes: u64,
    pub has_soft_mask: bool,
    /// True when this image is another image's soft mask.
    pub is_mask: bool,
    pub pages: Vec<u32>,
    /// Lowest pixels per inch at the largest size it is drawn. `None` when it was not found drawn.
    pub ppi: Option<f32>,
    /// Largest share of the image that falls inside the visible page, 0 to 1.
    /// Clipping paths are not taken into account, so the real share can be smaller.
    pub visible: Option<f32>,
    /// Why PDF Epsilon leaves it alone, when it does, as a code such as `cmyk`. See `imageReason.*`.
    pub unsupported: Option<String>,
    /// What each preset makes of it, in [`Preset::ALL`] order. `None` means unchanged.
    pub results: Vec<Option<ImageResult>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageResult {
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub downsampled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FontInfo {
    pub obj: u32,
    /// Base name, without a subset tag.
    pub name: String,
    pub kind: String,
    pub embedded: bool,
    pub subset: bool,
    /// The embedded font program alone.
    pub program_bytes: u64,
    /// Object number of the embedded program. Several font dictionaries can share one.
    pub program_obj: Option<u32>,
    /// Font dictionary, descriptor, program, widths and maps together. Parts shared
    /// with other fonts are split between them.
    pub bytes: u64,
    pub pages: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub name: String,
    pub bytes: u64,
}

/// Analyse the PDF at `input`. Nothing is written.
pub fn analyze_file(input: &std::path::Path) -> Result<Analysis> {
    let bytes = std::fs::read(input)?;
    analyze_bytes(&bytes)
}

/// Analyse a PDF held in memory.
pub fn analyze_bytes(input: &[u8]) -> Result<Analysis> {
    let started = Instant::now();
    let doc = crate::load(input)?;

    // Every preset runs on its own copy, at the same time as the inspection below.
    let (runs, mut a) = std::thread::scope(|s| {
        let handles: Vec<_> = Preset::ALL
            .iter()
            .map(|p| {
                let copy = doc.clone();
                s.spawn(move || crate::optimize(input, copy, &p.options()))
            })
            .collect();
        let inspected = Inspector::new(input, &doc).inspect();
        let runs: Vec<Result<Run>> = handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Err(Error::Verify("the preset run crashed".into()))))
            .collect();
        (runs, inspected)
    });

    a.apply_runs(&runs);
    let mut analysis = a.finish();
    analysis.millis = started.elapsed().as_millis() as u64;
    Ok(analysis)
}

// ---- inspection ----

struct Inspector<'a> {
    input: &'a [u8],
    doc: &'a Document,
    span: HashMap<ObjectId, u64>,
    category: HashMap<ObjectId, Category>,
    reachable: HashSet<ObjectId>,
    pages: BTreeMap<u32, ObjectId>,
}

/// Everything learned before the preset runs come in.
struct Inspected {
    file: FileInfo,
    span: HashMap<ObjectId, u64>,
    category: HashMap<ObjectId, Category>,
    overhead: u64,
    pages: Vec<PageInfo>,
    /// Per page: images drawn there, with their use count, and owned thumbnail and editor data ids.
    page_parts: Vec<PageParts>,
    images: Vec<ImageInfo>,
    image_ids: HashMap<u32, ObjectId>,
    fonts: Vec<FontInfo>,
    attachments: Vec<Attachment>,
    findings: Vec<Finding>,
    presets: Vec<PresetResult>,
    /// Content stream bytes stored without compression, and how many such streams.
    uncompressed: (u64, u32),
    /// XMP streams. The info dictionary is metadata too, but is never removed.
    xmp: u64,
}

#[derive(Default)]
struct PageParts {
    images: Vec<ObjectId>,
    thumbnail: Vec<ObjectId>,
    editor_data: Vec<ObjectId>,
}

impl<'a> Inspector<'a> {
    fn new(input: &'a [u8], doc: &'a Document) -> Self {
        Inspector {
            input,
            doc,
            span: HashMap::new(),
            category: HashMap::new(),
            reachable: HashSet::new(),
            pages: doc.get_pages(),
        }
    }

    fn inspect(mut self) -> Inspected {
        self.span = measure(self.input, self.doc);
        self.reachable = reachable(self.doc);
        self.category = classify(self.doc, &self.reachable, &self.pages);

        let attributed: u64 = self.span.values().sum();
        let overhead = (self.input.len() as u64).saturating_sub(attributed);
        let file = self.file_info();

        // Walk every page once.
        let mut walks = Vec::with_capacity(self.pages.len());
        for (&number, &page_id) in &self.pages {
            walks.push((number, page_id, walk_page(self.doc, page_id)));
        }

        let (images, image_ids) = self.images(&walks);
        let (fonts, font_parts) = self.fonts(&walks);
        let attachments = self.attachments();
        let (pages, page_parts, uncompressed) = self.pages(&walks, &images, &fonts, &font_parts);
        let mut file = file;
        file.unreadable_pages = walks.iter().filter(|w| !w.2.readable).map(|w| w.0).collect();
        let xmp = self
            .category
            .iter()
            .filter(|(id, c)| **c == Category::Metadata && matches!(self.doc.objects.get(id), Some(Object::Stream(_))))
            .map(|(id, _)| self.span_of(*id))
            .sum();

        Inspected {
            file,
            span: self.span,
            category: self.category,
            overhead,
            pages,
            page_parts,
            images,
            image_ids,
            fonts,
            attachments,
            findings: Vec::new(),
            presets: Vec::new(),
            uncompressed,
            xmp,
        }
    }

    fn span_of(&self, id: ObjectId) -> u64 {
        self.span.get(&id).copied().unwrap_or(0)
    }

    /// Bytes of `start` and of everything below it in the same category.
    fn owned(&self, start: &[ObjectId], category: Category) -> (u64, Vec<ObjectId>) {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<ObjectId> = start.iter().copied().collect();
        let mut total = 0;
        while let Some(id) = queue.pop_front() {
            if self.category.get(&id) != Some(&category) || !seen.insert(id) {
                continue;
            }
            total += self.span_of(id);
            if let Some(obj) = self.doc.objects.get(&id) {
                let mut kids = Vec::new();
                children(obj, &mut kids);
                queue.extend(kids);
            }
        }
        (total, seen.into_iter().collect())
    }

    fn file_info(&self) -> FileInfo {
        let doc = self.doc;
        let info = doc.trailer.get(b"Info").ok().and_then(|o| deref(doc, o).as_dict().ok());
        let text = |key: &[u8]| {
            info.and_then(|d| d.get(key).ok())
                .map(|o| deref(doc, o))
                .and_then(|o| lopdf::decode_text_string(o).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let catalog = doc.catalog().ok();
        let linearized = doc.objects.values().any(|o| o.as_dict().map(|d| d.has(b"Linearized")).unwrap_or(false));
        let startxrefs = count(self.input, b"startxref") as u32;
        let revisions = startxrefs.saturating_sub(linearized as u32).max(1);
        let tagged = catalog.map(|c| c.has(b"StructTreeRoot")).unwrap_or(false);
        let signed = doc.objects.values().any(|o| {
            o.as_dict().map(|d| d.has(b"ByteRange") && d.has(b"Contents")).unwrap_or(false)
        });
        let hidden_layers = catalog
            .and_then(|c| c.get(b"OCProperties").ok())
            .and_then(|o| deref(doc, o).as_dict().ok())
            .and_then(|p| p.get(b"D").ok())
            .and_then(|o| deref(doc, o).as_dict().ok())
            .and_then(|d| d.get(b"OFF").ok())
            .and_then(|o| deref(doc, o).as_array().ok())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let pdfa = catalog
            .and_then(|c| c.get(b"Metadata").ok())
            .and_then(|o| o.as_reference().ok())
            .and_then(|id| doc.get_object(id).ok())
            .and_then(|o| o.as_stream().ok())
            .and_then(|s| s.decompressed_content().ok().or_else(|| Some(s.content.clone())))
            .and_then(|xmp| pdfa_level(&xmp));
        FileInfo {
            bytes: self.input.len() as u64,
            pages: self.pages.len() as u32,
            version: doc.version.clone(),
            producer: text(b"Producer"),
            creator: text(b"Creator"),
            objects: doc.objects.len() as u32,
            revisions,
            linearized,
            object_streams: doc.reference_table.entries.values().any(|e| matches!(e, XrefEntry::Compressed { .. })),
            tagged,
            signed,
            pdfa,
            hidden_layers,
            unreadable_pages: Vec::new(),
        }
    }

    fn images(&self, walks: &[(u32, ObjectId, PageWalk)]) -> (Vec<ImageInfo>, HashMap<u32, ObjectId>) {
        let doc = self.doc;
        // Soft and explicit masks belong to their parent's placements.
        let mut parent_of: HashMap<ObjectId, ObjectId> = HashMap::new();
        for (id, obj) in &doc.objects {
            if let Object::Stream(s) = obj {
                if is_image(&s.dict) {
                    for key in [&b"SMask"[..], b"Mask"] {
                        if let Ok(Object::Reference(m)) = s.dict.get(key) {
                            parent_of.insert(*m, *id);
                        }
                    }
                }
            }
        }

        struct Seen {
            pages: BTreeSet<u32>,
            drawn: (f32, f32),
            visible: f32,
        }
        let mut seen: HashMap<ObjectId, Seen> = HashMap::new();
        for (number, page_id, walk) in walks {
            let page = placement::visible_box(doc, *page_id);
            for (id, m) in &walk.images {
                let (w, h) = m.unit_size();
                let visible = page.map(|p| visible_share(m.unit_bounds(), p)).unwrap_or(1.0);
                let e = seen.entry(*id).or_insert(Seen { pages: BTreeSet::new(), drawn: (0.0, 0.0), visible: 0.0 });
                e.pages.insert(*number);
                e.drawn = (e.drawn.0.max(w), e.drawn.1.max(h));
                e.visible = e.visible.max(visible);
            }
        }

        let mut out = Vec::new();
        let mut ids = HashMap::new();
        for (id, obj) in &doc.objects {
            let Object::Stream(s) = obj else { continue };
            if !is_image(&s.dict) || !self.reachable.contains(id) {
                continue;
            }
            let dict = &s.dict;
            let width = images::int(doc, dict, b"Width").unwrap_or(0).max(0) as u32;
            let height = images::int(doc, dict, b"Height").unwrap_or(0).max(0) as u32;
            let is_mask = parent_of.contains_key(id);
            let placed = seen.get(id).or_else(|| parent_of.get(id).and_then(|p| seen.get(p)));
            let ppi = placed.filter(|p| p.drawn.0 >= 1.0 && p.drawn.1 >= 1.0).map(|p| {
                (width as f32 / (p.drawn.0 / 72.0)).min(height as f32 / (p.drawn.1 / 72.0))
            });
            let jpeg = matches!(dict.get(b"Filter"), Ok(Object::Name(n)) if n == b"DCTDecode");
            let bits = if jpeg { 8 } else { images::int(doc, dict, b"BitsPerComponent").unwrap_or(8) as u32 };
            ids.insert(id.0, *id);
            out.push(ImageInfo {
                obj: id.0,
                width,
                height,
                bits,
                colour: if dict.get(b"ImageMask").and_then(|o| o.as_bool()).unwrap_or(false) {
                    "Stencil".into()
                } else if is_mask && !dict.has(b"ColorSpace") {
                    "Mask".into()
                } else {
                    images::colour_name(doc, dict)
                },
                encoding: images::encoding_name(dict),
                bytes: self.span_of(*id),
                has_soft_mask: dict.has(b"SMask"),
                is_mask,
                pages: placed.map(|p| p.pages.iter().copied().collect()).unwrap_or_default(),
                ppi,
                visible: placed.map(|p| p.visible),
                unsupported: images::unsupported(doc, dict, is_mask).map(String::from),
                results: vec![None; Preset::ALL.len()],
            });
        }
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.obj.cmp(&b.obj)));
        (out, ids)
    }

    /// The fonts, and the objects that make up each one.
    fn fonts(&self, walks: &[(u32, ObjectId, PageWalk)]) -> (Vec<FontInfo>, HashMap<ObjectId, Vec<ObjectId>>) {
        let doc = self.doc;
        let mut used: BTreeMap<ObjectId, BTreeSet<u32>> = BTreeMap::new();
        for (number, page_id, walk) in walks {
            for id in &walk.fonts {
                used.entry(*id).or_default().insert(*number);
            }
            // Fonts a page lists but never selects still take space; record them too.
            for id in resource_fonts(doc, *page_id) {
                used.entry(id).or_default();
            }
        }

        let parts: HashMap<ObjectId, Vec<ObjectId>> =
            used.keys().map(|id| (*id, self.owned(&[*id], Category::Fonts).1)).collect();
        let mut sharing: HashMap<ObjectId, u64> = HashMap::new();
        for ids in parts.values() {
            for id in ids {
                *sharing.entry(*id).or_default() += 1;
            }
        }

        let mut out = Vec::new();
        for (id, pages) in used {
            let Ok(dict) = doc.get_dictionary(id) else { continue };
            let subtype = name_of(dict.get(b"Subtype").ok()).unwrap_or_default();
            let base = name_of(dict.get(b"BaseFont").ok()).unwrap_or_else(|| "(unnamed)".into());
            // A Type 0 font keeps its descriptor in its descendant.
            let descendant = (subtype == "Type0")
                .then(|| dict.get(b"DescendantFonts").ok())
                .flatten()
                .and_then(|o| deref(doc, o).as_array().ok())
                .and_then(|a| a.first())
                .and_then(|o| deref(doc, o).as_dict().ok());
            let kind = match descendant {
                Some(d) => format!("Type0 / {}", name_of(d.get(b"Subtype").ok()).unwrap_or_default()),
                None => subtype.clone(),
            };
            let descriptor = descendant
                .unwrap_or(dict)
                .get(b"FontDescriptor")
                .ok()
                .and_then(|o| deref(doc, o).as_dict().ok());
            let program = descriptor.and_then(|d| {
                [&b"FontFile"[..], b"FontFile2", b"FontFile3"]
                    .iter()
                    .find_map(|k| d.get(k).ok().and_then(|o| o.as_reference().ok()))
            });
            let bytes = parts[&id].iter().map(|p| self.span_of(*p) / sharing[p]).sum();
            let subset = base.len() > 7 && base.as_bytes()[6] == b'+' && base[..6].bytes().all(|b| b.is_ascii_uppercase());
            out.push(FontInfo {
                obj: id.0,
                name: if subset { base[7..].to_string() } else { base },
                kind,
                embedded: program.is_some() || subtype == "Type3",
                subset,
                program_bytes: program.map(|p| self.span_of(p)).unwrap_or(0),
                program_obj: program.map(|p| p.0),
                bytes,
                pages: pages.into_iter().collect(),
            });
        }
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.obj.cmp(&b.obj)));
        (out, parts)
    }

    fn attachments(&self) -> Vec<Attachment> {
        let doc = self.doc;
        let mut out = Vec::new();
        for (id, obj) in &doc.objects {
            let Object::Dictionary(d) = obj else { continue };
            if !self.reachable.contains(id) {
                continue;
            }
            let Ok(ef) = d.get(b"EF") else { continue };
            let Ok(ef) = deref(doc, ef).as_dict() else { continue };
            let Some(file) = ef.get(b"UF").or_else(|_| ef.get(b"F")).ok().and_then(|o| o.as_reference().ok()) else {
                continue;
            };
            let name = d
                .get(b"UF")
                .or_else(|_| d.get(b"F"))
                .ok()
                .and_then(|o| lopdf::decode_text_string(deref(doc, o)).ok())
                .unwrap_or_else(|| "(unnamed)".into());
            out.push(Attachment { name, bytes: self.span_of(file) });
        }
        out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        out
    }

    fn pages(
        &self,
        walks: &[(u32, ObjectId, PageWalk)],
        images: &[ImageInfo],
        fonts: &[FontInfo],
        font_parts: &HashMap<ObjectId, Vec<ObjectId>>,
    ) -> (Vec<PageInfo>, Vec<PageParts>, (u64, u32)) {
        let doc = self.doc;
        let image_by_obj: HashMap<u32, &ImageInfo> = images.iter().map(|i| (i.obj, i)).collect();
        let font_by_obj: HashMap<u32, &FontInfo> = fonts.iter().map(|f| (f.obj, f)).collect();
        // How many pages use each form, so shared ones are split fairly.
        let mut form_uses: HashMap<ObjectId, u32> = HashMap::new();
        for (_, _, w) in walks {
            for f in &w.forms {
                *form_uses.entry(*f).or_default() += 1;
            }
        }
        // How many pages use each font object, counted once per page however many fonts share it.
        let page_font_objects = |walk: &PageWalk| -> HashSet<ObjectId> {
            walk.fonts.iter().filter_map(|f| font_parts.get(f)).flatten().copied().collect()
        };
        let mut font_object_pages: HashMap<ObjectId, u64> = HashMap::new();
        for (_, _, w) in walks {
            for id in page_font_objects(w) {
                *font_object_pages.entry(id).or_default() += 1;
            }
        }
        let big_content = (self.input.len() as u64 / 5).max(1 << 20);
        let mut uncompressed = (0u64, 0u32);

        let mut pages = Vec::new();
        let mut parts = Vec::new();
        for (number, page_id, walk) in walks {
            let page = doc.get_dictionary(*page_id).ok();
            let b = placement::visible_box(doc, *page_id).unwrap_or([0.0, 0.0, 612.0, 792.0]);
            let mut info = PageInfo {
                number: *number,
                width: b[2] - b[0],
                height: b[3] - b[1],
                bytes: 0,
                content: 0,
                images: 0,
                fonts: 0,
                graphics: 0,
                other: 0,
                image_objs: Vec::new(),
                font_objs: Vec::new(),
                issues: Vec::new(),
                savings: vec![0; Preset::ALL.len()],
            };
            let mut part = PageParts::default();

            // Content streams.
            let mut raw_content = 0;
            for id in doc.get_page_contents(*page_id) {
                let len = self.span_of(id);
                info.content += len;
                if let Ok(Object::Stream(s)) = doc.get_object(id) {
                    if !s.dict.has(b"Filter") && s.content.len() >= 4096 {
                        raw_content += s.content.len() as u64;
                    }
                }
            }
            if raw_content > 0 {
                uncompressed.0 += raw_content;
                uncompressed.1 += 1;
                info.issues.push(PageIssue {
                    kind: "uncompressedContent",
                    status: Status::Fixed,
                    message: msg!("issue.uncompressedContent", size = Bytes(raw_content)),
                    bytes: raw_content,
                });
            }
            if info.content >= big_content {
                info.issues.push(PageIssue {
                    kind: "heavyContent",
                    status: Status::NotYet,
                    message: msg!("issue.heavyContent", size = Bytes(info.content)),
                    bytes: info.content,
                });
            }

            // Images, with their masks.
            let mut drawn: BTreeSet<ObjectId> = walk.images.iter().map(|(id, _)| *id).collect();
            for id in drawn.clone() {
                if let Ok(Object::Stream(s)) = doc.get_object(id) {
                    for key in [&b"SMask"[..], b"Mask"] {
                        if let Ok(Object::Reference(m)) = s.dict.get(key) {
                            drawn.insert(*m);
                        }
                    }
                }
            }
            let mut oversized: Vec<(u64, Message)> = Vec::new();
            for id in &drawn {
                let Some(img) = image_by_obj.get(&id.0) else { continue };
                let uses = img.pages.len().max(1) as u64;
                info.images += img.bytes / uses;
                info.image_objs.push(id.0);
                part.images.push(*id);
                if img.is_mask {
                    continue;
                }
                let share = img.bytes / uses;
                let colour = || Term(format!("colour.{}", img.colour));
                if let Some(reason) = &img.unsupported {
                    if img.bytes >= 50_000 {
                        info.issues.push(PageIssue {
                            kind: "unsupportedImage",
                            status: Status::NotYet,
                            message: msg!(
                                "issue.unsupportedImage",
                                width = img.width,
                                height = img.height,
                                colour = colour(),
                                size = Bytes(img.bytes),
                                reason = Term(format!("imageReason.{reason}")),
                            ),
                            bytes: share,
                        });
                    }
                } else if let Some(ppi) = img.ppi.filter(|p| *p > 150.0 * 1.25) {
                    let message = if img.pages.len() > 1 {
                        msg!(
                            "issue.oversizedImageShared",
                            width = img.width,
                            height = img.height,
                            colour = colour(),
                            size = Bytes(img.bytes),
                            ppi = ppi.round(),
                            pages = img.pages.len(),
                        )
                    } else {
                        msg!(
                            "issue.oversizedImage",
                            width = img.width,
                            height = img.height,
                            colour = colour(),
                            size = Bytes(img.bytes),
                            ppi = ppi.round(),
                        )
                    };
                    oversized.push((share, message));
                }
                if let Some(v) = img.visible.filter(|v| *v < 0.7) {
                    if img.bytes >= 50_000 {
                        info.issues.push(PageIssue {
                            kind: "offpageImage",
                            status: Status::NotYet,
                            message: msg!("issue.offpageImage", percent = (v * 100.0).round(), size = Bytes(img.bytes)),
                            bytes: share,
                        });
                    }
                }
            }
            oversized.sort_by(|a, b| b.0.cmp(&a.0));
            let listed = oversized.iter().take_while(|o| o.0 >= 20_000).take(3).count();
            for (share, message) in oversized.drain(..listed) {
                info.issues.push(PageIssue { kind: "oversizedImage", status: Status::Fixed, message, bytes: share });
            }
            if !oversized.is_empty() {
                let rest: u64 = oversized.iter().map(|o| o.0).sum();
                info.issues.push(PageIssue {
                    kind: "oversizedImage",
                    status: Status::Fixed,
                    message: if listed > 0 {
                        msg!("issue.oversizedMore", count = oversized.len(), size = Bytes(rest))
                    } else {
                        msg!("issue.oversizedImages", count = oversized.len(), size = Bytes(rest))
                    },
                    bytes: rest,
                });
            }
            if walk.inline_images.0 > 0 && walk.inline_images.1 >= 50_000 {
                info.issues.push(PageIssue {
                    kind: "inlineImages",
                    status: Status::NotYet,
                    message: msg!("issue.inlineImages", count = walk.inline_images.0, size = Bytes(walk.inline_images.1)),
                    bytes: walk.inline_images.1,
                });
            }

            // Forms.
            for id in &walk.forms {
                let uses = form_uses.get(id).copied().unwrap_or(1).max(1) as u64;
                info.graphics += self.span_of(*id) / uses;
            }

            // Fonts. A font not subset is flagged on the first page that uses it.
            let mut page_fonts: Vec<&FontInfo> =
                walk.fonts.iter().filter_map(|id| font_by_obj.get(&id.0).copied()).collect();
            page_fonts.sort_by_key(|f| f.obj);
            page_fonts.dedup_by_key(|f| f.obj);
            info.fonts = page_font_objects(walk).iter().map(|id| self.span_of(*id) / font_object_pages[id]).sum();
            for f in page_fonts {
                info.font_objs.push(f.obj);
                if f.embedded && !f.subset && f.program_bytes >= 20_000 && f.pages.first() == Some(number) {
                    info.issues.push(PageIssue {
                        kind: "fullFont",
                        status: Status::NotYet,
                        message: msg!("issue.fullFont", name = f.name.as_str(), size = Bytes(f.program_bytes)),
                        bytes: f.program_bytes,
                    });
                }
            }

            // Annotations, thumbnail and editor data owned by the page.
            if let Some(page) = page {
                let refs = |key: &[u8]| -> Vec<ObjectId> {
                    let mut v = Vec::new();
                    if let Ok(o) = page.get(key) {
                        children(o, &mut v);
                        if let Ok(a) = deref(doc, o).as_array() {
                            a.iter().for_each(|x| children(x, &mut v));
                        }
                    }
                    v
                };
                let (annots, _) = self.owned(&refs(b"Annots"), Category::Annotations);
                let (thumb, thumb_ids) = self.owned(&refs(b"Thumb"), Category::Thumbnails);
                let (editor, editor_ids) = self.owned(&refs(b"PieceInfo"), Category::EditorData);
                info.other = annots + thumb + editor;
                if thumb > 0 {
                    info.issues.push(PageIssue {
                        kind: "thumbnail",
                        status: Status::Fixed,
                        message: msg!("issue.thumbnail", size = Bytes(thumb)),
                        bytes: thumb,
                    });
                }
                if editor >= 1024 {
                    info.issues.push(PageIssue {
                        kind: "editorData",
                        status: Status::Fixed,
                        message: msg!("issue.editorData", size = Bytes(editor)),
                        bytes: editor,
                    });
                }
                part.thumbnail = thumb_ids;
                part.editor_data = editor_ids;
            }
            if !walk.readable {
                info.issues.push(PageIssue {
                    kind: "unreadable",
                    status: Status::Warning,
                    message: msg!("issue.unreadable"),
                    bytes: 0,
                });
            }

            info.bytes = info.content + info.images + info.fonts + info.graphics + info.other;
            info.issues.sort_by(|a, b| b.bytes.cmp(&a.bytes));
            pages.push(info);
            parts.push(part);
        }
        (pages, parts, uncompressed)
    }
}

// ---- after the preset runs ----

impl Inspected {
    fn apply_runs(&mut self, runs: &[Result<Run>]) {
        let input = self.file.bytes;
        for (i, (preset, run)) in Preset::ALL.iter().zip(runs).enumerate() {
            let run = match run {
                Ok(r) => r,
                Err(e) => {
                    self.presets.push(PresetResult {
                        preset: *preset,
                        output_bytes: input,
                        kept_original: true,
                        error: Some(e.to_string()),
                        savings: Vec::new(),
                        images_changed: 0,
                    });
                    continue;
                }
            };
            if run.report.kept_original {
                self.presets.push(PresetResult {
                    preset: *preset,
                    output_bytes: input,
                    kept_original: true,
                    error: None,
                    savings: Vec::new(),
                    images_changed: 0,
                });
                continue;
            }

            let image_saving: HashMap<ObjectId, i64> =
                run.images.iter().map(|c| (c.id, c.before as i64 - c.after as i64)).collect();
            for c in &run.images {
                if let Some(img) = self.images.iter_mut().find(|im| im.obj == c.id.0 && self.image_ids.get(&im.obj) == Some(&c.id)) {
                    img.results[i] = Some(ImageResult {
                        bytes: c.after as u64,
                        width: c.width,
                        height: c.height,
                        downsampled: c.downsampled,
                    });
                }
            }
            let removed_span = |id: &ObjectId| {
                self.span.get(id).copied().unwrap_or(0) as i64 - image_saving.get(id).copied().unwrap_or(0).max(0)
            };
            let mut by_kind: Vec<(SavingKind, i64)> = vec![
                (SavingKind::Images, image_saving.values().sum()),
                (SavingKind::Duplicates, run.merged.iter().map(removed_span).sum()),
            ];
            for (kind, cat) in [
                (SavingKind::Thumbnails, Category::Thumbnails),
                (SavingKind::EditorData, Category::EditorData),
                (SavingKind::Metadata, Category::Metadata),
                (SavingKind::Unused, Category::Unused),
            ] {
                let total: i64 = run.pruned.iter().filter(|id| self.category.get(id) == Some(&cat)).map(removed_span).sum();
                by_kind.push((kind, total));
            }
            let saved = input as i64 - run.report.output_bytes as i64;
            let accounted: i64 = by_kind.iter().map(|k| k.1).sum();
            by_kind.push((SavingKind::Rewrite, saved - accounted));

            // Page by page: images split across the pages that show them, plus removed thumbnails and editor data.
            let pruned: HashSet<ObjectId> = run.pruned.iter().copied().collect();
            for (page, part) in self.pages.iter_mut().zip(&self.page_parts) {
                let mut s: i64 = 0;
                for id in &part.images {
                    let uses = self.images.iter().find(|im| im.obj == id.0).map(|im| im.pages.len().max(1)).unwrap_or(1);
                    s += image_saving.get(id).copied().unwrap_or(0) / uses as i64;
                }
                for id in part.thumbnail.iter().chain(&part.editor_data) {
                    if pruned.contains(id) {
                        s += self.span.get(id).copied().unwrap_or(0) as i64;
                    }
                }
                page.savings[i] = s.max(0) as u64;
            }

            self.presets.push(PresetResult {
                preset: *preset,
                output_bytes: run.report.output_bytes,
                kept_original: false,
                error: None,
                savings: by_kind.into_iter().filter(|k| k.1 != 0).map(|(kind, bytes)| Saving { kind, bytes }).collect(),
                images_changed: run.images.len() as u32,
            });
        }
        self.findings = self.findings(runs);
    }

    fn category_bytes(&self, cat: Category) -> u64 {
        self.category.iter().filter(|(_, c)| **c == cat).map(|(id, _)| self.span.get(id).copied().unwrap_or(0)).sum()
    }

    fn saving(&self, i: usize, kind: SavingKind) -> u64 {
        self.presets
            .get(i)
            .and_then(|p| p.savings.iter().find(|s| s.kind == kind))
            .map(|s| s.bytes.max(0) as u64)
            .unwrap_or(0)
    }

    fn savings_of(&self, kind: SavingKind) -> Vec<u64> {
        (0..Preset::ALL.len()).map(|i| self.saving(i, kind)).collect()
    }

    fn findings(&self, runs: &[Result<Run>]) -> Vec<Finding> {
        let total = self.file.bytes.max(1);
        let severity = |b: u64| match b * 100 / total {
            10.. => Severity::High,
            2..=9 => Severity::Medium,
            _ => Severity::Low,
        };
        let mut out: Vec<Finding> = Vec::new();
        let none = || vec![0; Preset::ALL.len()];
        let mut push = |kind, status, title: Message, detail: Vec<Message>, b: u64, savings: Vec<u64>, pages: Vec<u32>| {
            let best = savings.iter().copied().max().unwrap_or(0);
            let sev = match status {
                Status::Warning | Status::Kept => Severity::Info,
                Status::Fixed if best > 0 => severity(best),
                _ => severity(b),
            };
            out.push(Finding { kind, severity: sev, status, title, detail, bytes: b, savings, pages });
        };
        let pages_of = |imgs: &[&ImageInfo]| -> Vec<u32> {
            imgs.iter().flat_map(|i| i.pages.iter().copied()).collect::<BTreeSet<_>>().into_iter().collect()
        };
        let image_savings = |imgs: &[&ImageInfo]| -> Vec<u64> {
            (0..Preset::ALL.len())
                .map(|p| {
                    imgs.iter()
                        .filter_map(|im| im.results[p].as_ref().map(|r| im_stream_len(runs, p, im.obj).saturating_sub(r.bytes)))
                        .sum()
                })
                .collect()
        };

        // Images drawn at more resolution than any reader needs.
        let downsampled_by = |im: &ImageInfo, p: Preset| {
            let i = Preset::ALL.iter().position(|x| *x == p).unwrap();
            im.results[i].as_ref().map(|r| r.downsampled).unwrap_or(false)
        };
        let oversized: Vec<&ImageInfo> = self
            .images
            .iter()
            .filter(|im| downsampled_by(im, Preset::Print) || downsampled_by(im, Preset::Balanced))
            .collect();
        if !oversized.is_empty() {
            let b = oversized.iter().map(|i| i.bytes).sum();
            let top = oversized.iter().filter_map(|i| i.ppi).fold(0.0f32, f32::max);
            push(
                "oversizedImages",
                Status::Fixed,
                msg!("finding.oversizedImages.title", count = oversized.len()),
                vec![msg!("finding.oversizedImages.detail", ppi = top.round())],
                b,
                image_savings(&oversized),
                pages_of(&oversized),
            );
        }
        // Images whose resolution suits the screen, but that still shrink.
        let recodable: Vec<&ImageInfo> = self
            .images
            .iter()
            .filter(|im| {
                im.results.iter().any(|r| r.is_some())
                    && !downsampled_by(im, Preset::Print)
                    && !downsampled_by(im, Preset::Balanced)
            })
            .collect();
        if !recodable.is_empty() {
            let b = recodable.iter().map(|i| i.bytes).sum();
            push(
                "recodableImages",
                Status::Fixed,
                msg!("finding.recodableImages.title", count = recodable.len()),
                vec![msg!("finding.recodableImages.detail")],
                b,
                image_savings(&recodable),
                pages_of(&recodable),
            );
        }
        // Images PDF Epsilon cannot handle yet, grouped by reason.
        let mut reasons: BTreeMap<&str, Vec<&ImageInfo>> = BTreeMap::new();
        for im in &self.images {
            if let Some(r) = &im.unsupported {
                reasons.entry(r.as_str()).or_default().push(im);
            }
        }
        for (reason, imgs) in reasons {
            let b: u64 = imgs.iter().map(|i| i.bytes).sum();
            if b < 100_000 && b * 100 < total {
                continue;
            }
            let over = imgs.iter().filter(|i| i.ppi.map(|p| p > 187.5).unwrap_or(false)).count();
            let term = || Term(format!("imageReason.{reason}"));
            let mut detail = vec![msg!("finding.unsupportedImages.detail", reason = term())];
            if over > 0 {
                detail.push(msg!("finding.unsupportedImages.over", count = over));
            }
            push(
                "unsupportedImages",
                Status::NotYet,
                msg!("finding.unsupportedImages.title", count = imgs.len(), reason = term()),
                detail,
                b,
                none(),
                pages_of(&imgs),
            );
        }
        // Images mostly outside the visible page.
        let offpage: Vec<&ImageInfo> =
            self.images.iter().filter(|i| i.bytes >= 50_000 && i.visible.map(|v| v < 0.7).unwrap_or(false)).collect();
        if !offpage.is_empty() {
            let b = offpage.iter().map(|i| i.bytes).sum();
            let least = offpage.iter().filter_map(|i| i.visible).fold(1.0f32, f32::min);
            push(
                "offpageImages",
                Status::NotYet,
                msg!("finding.offpageImages.title", count = offpage.len()),
                vec![msg!("finding.offpageImages.detail", percent = (least * 100.0).round())],
                b,
                none(),
                pages_of(&offpage),
            );
        }
        // Images never drawn on any page.
        let undrawn: Vec<&ImageInfo> = self.images.iter().filter(|i| i.pages.is_empty() && !i.is_mask).collect();
        let undrawn_bytes: u64 = undrawn.iter().map(|i| i.bytes).sum();
        if undrawn_bytes >= 50_000 {
            push(
                "undrawnImages",
                Status::Kept,
                msg!("finding.undrawnImages.title", count = undrawn.len()),
                vec![msg!("finding.undrawnImages.detail")],
                undrawn_bytes,
                none(),
                Vec::new(),
            );
        }

        // Fonts.
        let full: Vec<&FontInfo> = self.fonts.iter().filter(|f| f.embedded && !f.subset && f.program_bytes >= 20_000).collect();
        if !full.is_empty() {
            let b = full.iter().map(|f| f.program_bytes).sum();
            let names: Vec<String> = full.iter().take(4).map(|f| f.name.clone()).collect();
            let names = if full.len() > 4 {
                msg!("finding.fullFonts.namesAndMore", names = names, count = full.len() - 4)
            } else {
                msg!("finding.fullFonts.names", names = names)
            };
            push(
                "fullFonts",
                Status::NotYet,
                msg!("finding.fullFonts.title", count = full.len()),
                vec![names, msg!("finding.fullFonts.detail")],
                b,
                none(),
                full.iter().flat_map(|f| f.pages.first().copied()).collect::<BTreeSet<_>>().into_iter().collect(),
            );
        }
        // Counted by distinct program: some producers point many font dictionaries at one.
        let mut families: BTreeMap<&str, Vec<&FontInfo>> = BTreeMap::new();
        let mut programs = HashSet::new();
        for f in self.fonts.iter().filter(|f| f.program_bytes > 0) {
            if programs.insert(f.program_obj) {
                families.entry(f.name.as_str()).or_default().push(f);
            }
        }
        let repeated: Vec<(&str, Vec<&FontInfo>)> = families.into_iter().filter(|(_, v)| v.len() > 1).collect();
        let repeated_bytes: u64 = repeated
            .iter()
            .map(|(_, v)| v.iter().map(|f| f.program_bytes).sum::<u64>() - v.iter().map(|f| f.program_bytes).max().unwrap_or(0))
            .sum();
        if repeated_bytes >= 20_000 {
            let copies: usize = repeated.iter().map(|(_, v)| v.len()).sum();
            let examples: Vec<String> = repeated.iter().take(4).map(|(n, v)| format!("{n} ×{}", v.len())).collect();
            push(
                "repeatedFonts",
                Status::NotYet,
                msg!("finding.repeatedFonts.title", count = repeated.len(), copies = copies),
                vec![msg!("finding.repeatedFonts.detail", fonts = examples)],
                repeated_bytes,
                none(),
                Vec::new(),
            );
        }

        // Structure-level clutter removed by presets.
        let dup = self.savings_of(SavingKind::Duplicates);
        if dup.iter().any(|b| *b > 0) {
            let merged = runs.first().and_then(|r| r.as_ref().ok()).map(|r| r.merged.len()).unwrap_or(0);
            push(
                "duplicates",
                Status::Fixed,
                msg!("finding.duplicates.title", count = merged),
                vec![msg!("finding.duplicates.detail")],
                *dup.iter().max().unwrap(),
                dup,
                Vec::new(),
            );
        }
        let thumbs = self.category_bytes(Category::Thumbnails);
        if thumbs > 0 {
            push(
                "thumbnails",
                Status::Fixed,
                msg!("finding.thumbnails.title"),
                vec![msg!("finding.thumbnails.detail")],
                thumbs,
                self.savings_of(SavingKind::Thumbnails),
                self.pages.iter().filter(|p| p.issues.iter().any(|i| i.kind == "thumbnail")).map(|p| p.number).collect(),
            );
        }
        let editor = self.category_bytes(Category::EditorData);
        if editor >= 1024 {
            push(
                "editorData",
                Status::Fixed,
                msg!("finding.editorData.title"),
                vec![msg!("finding.editorData.detail")],
                editor,
                self.savings_of(SavingKind::EditorData),
                self.pages.iter().filter(|p| p.issues.iter().any(|i| i.kind == "editorData")).map(|p| p.number).collect(),
            );
        }
        let xmp = self.xmp;
        if xmp >= 4096 {
            let mut detail = vec![msg!("finding.metadata.detail")];
            if self.file.pdfa.is_some() {
                detail.push(msg!("finding.metadata.pdfa"));
            }
            push(
                "metadata",
                Status::Fixed,
                msg!("finding.metadata.title"),
                detail,
                xmp,
                self.savings_of(SavingKind::Metadata),
                Vec::new(),
            );
        }
        let unused = self.category_bytes(Category::Unused);
        if unused >= 1024 {
            push(
                "unusedObjects",
                Status::Fixed,
                msg!("finding.unusedObjects.title"),
                vec![msg!("finding.unusedObjects.detail")],
                unused,
                self.savings_of(SavingKind::Unused),
                Vec::new(),
            );
        }
        let rewrite = self.savings_of(SavingKind::Rewrite);
        let rewrite_top = rewrite.iter().copied().max().unwrap_or(0);
        if rewrite_top * 200 >= total {
            let (title, mut detail) = if self.file.revisions > 1 {
                (
                    msg!("finding.rewrite.revisionsTitle", count = self.file.revisions),
                    vec![msg!("finding.rewrite.revisionsDetail", size = Bytes(self.overhead))],
                )
            } else {
                (msg!("finding.rewrite.title"), vec![msg!("finding.rewrite.detail")])
            };
            if !self.file.object_streams {
                detail.push(msg!("finding.rewrite.noObjectStreams"));
            }
            if self.uncompressed.1 > 0 {
                detail.push(msg!("finding.rewrite.uncompressed", size = Bytes(self.uncompressed.0)));
            }
            push("rewrite", Status::Fixed, title, detail, rewrite_top, rewrite, Vec::new());
        }
        let heavy: Vec<&PageInfo> = self.pages.iter().filter(|p| p.issues.iter().any(|i| i.kind == "heavyContent")).collect();
        if !heavy.is_empty() {
            push(
                "heavyContent",
                Status::NotYet,
                msg!("finding.heavyContent.title", count = heavy.len()),
                vec![msg!("finding.heavyContent.detail")],
                heavy.iter().map(|p| p.content).sum(),
                none(),
                heavy.iter().map(|p| p.number).collect(),
            );
        }

        // Kept on purpose, and warnings.
        if !self.attachments.is_empty() {
            let b = self.attachments.iter().map(|a| a.bytes).sum();
            let names: Vec<String> = self.attachments.iter().take(4).map(|a| a.name.clone()).collect();
            push(
                "attachments",
                Status::Kept,
                msg!("finding.attachments.title", count = self.attachments.len()),
                vec![msg!("finding.attachments.detail", names = names)],
                b,
                none(),
                Vec::new(),
            );
        }
        let tags = self.category_bytes(Category::Tagging);
        if tags * 100 >= total {
            push(
                "tagging",
                Status::Kept,
                msg!("finding.tagging.title"),
                vec![msg!("finding.tagging.detail")],
                tags,
                none(),
                Vec::new(),
            );
        }
        if self.file.signed {
            push(
                "signed",
                Status::Warning,
                msg!("finding.signed.title"),
                vec![msg!("finding.signed.detail")],
                0,
                none(),
                Vec::new(),
            );
        }
        if let Some(level) = &self.file.pdfa {
            push(
                "pdfa",
                Status::Warning,
                msg!("finding.pdfa.title", level = level.as_str()),
                vec![msg!("finding.pdfa.detail")],
                0,
                none(),
                Vec::new(),
            );
        }
        if self.file.hidden_layers {
            push(
                "hiddenLayers",
                Status::Warning,
                msg!("finding.hiddenLayers.title"),
                vec![msg!("finding.hiddenLayers.detail")],
                0,
                none(),
                Vec::new(),
            );
        }
        for (p, r) in Preset::ALL.iter().zip(runs) {
            if let Err(e) = r {
                let preset = format!("{p:?}").to_lowercase();
                push(
                    "presetFailed",
                    Status::Warning,
                    msg!("finding.presetFailed.title", preset = Term(format!("preset.{preset}"))),
                    vec![msg!("finding.presetFailed.detail", code = Term(format!("error.{}", e.code())), detail = e.detail())],
                    0,
                    none(),
                    Vec::new(),
                );
            }
        }

        let rank = |s: Status| match s {
            Status::Warning => 0,
            _ => 1,
        };
        out.sort_by(|a, b| rank(a.status).cmp(&rank(b.status)).then(b.bytes.cmp(&a.bytes)));
        out
    }

    fn finish(self) -> Analysis {
        let mut totals: BTreeMap<Category, (u64, u32)> = BTreeMap::new();
        for (id, cat) in &self.category {
            let e = totals.entry(*cat).or_default();
            e.0 += self.span.get(id).copied().unwrap_or(0);
            e.1 += 1;
        }
        let e = totals.entry(Category::Overhead).or_default();
        e.0 += self.overhead;
        let mut categories: Vec<CategoryUsage> = totals
            .into_iter()
            .filter(|(_, (b, _))| *b > 0)
            .map(|(category, (bytes, objects))| CategoryUsage { category, bytes, objects })
            .collect();
        categories.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        Analysis {
            file: self.file,
            categories,
            presets: self.presets,
            findings: self.findings,
            pages: self.pages,
            images: self.images,
            fonts: self.fonts,
            attachments: self.attachments,
            millis: 0,
        }
    }
}

/// The encoded length of an image before a run changed it.
fn im_stream_len(runs: &[Result<Run>], preset: usize, obj: u32) -> u64 {
    runs.get(preset)
        .and_then(|r| r.as_ref().ok())
        .and_then(|r| r.images.iter().find(|c| c.id.0 == obj))
        .map(|c| c.before as u64)
        .unwrap_or(0)
}

// ---- page walking ----

#[derive(Default)]
struct PageWalk {
    images: Vec<(ObjectId, placement::Matrix)>,
    forms: BTreeSet<ObjectId>,
    fonts: BTreeSet<ObjectId>,
    /// Count and encoded bytes.
    inline_images: (usize, u64),
    readable: bool,
}

fn walk_page(doc: &Document, page_id: ObjectId) -> PageWalk {
    let mut w = PageWalk::default();
    let readable = placement::visit_page(doc, page_id, &mut |draw| match draw {
        Draw::Image(id, m) => w.images.push((id, m)),
        Draw::Form(id) => {
            w.forms.insert(id);
        }
        Draw::Font(id) => {
            w.fonts.insert(id);
        }
        Draw::InlineImage(s) => {
            w.inline_images.0 += 1;
            w.inline_images.1 += s.content.len() as u64;
        }
    });
    w.readable = readable;
    w
}

/// Share of the bounds `b` inside the page box `p`, both as x0, y0, x1, y1.
fn visible_share(b: [f32; 4], p: [f32; 4]) -> f32 {
    let area = (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
    if area <= 0.0 {
        return 1.0;
    }
    let w = (b[2].min(p[2]) - b[0].max(p[0])).max(0.0);
    let h = (b[3].min(p[3]) - b[1].max(p[1])).max(0.0);
    (w * h / area).clamp(0.0, 1.0)
}

// ---- measuring ----

/// On-disk bytes of every object. Objects inside an object stream share its size in
/// proportion to their length.
fn measure(bytes: &[u8], doc: &Document) -> HashMap<ObjectId, u64> {
    let ids: HashMap<u32, ObjectId> = doc.objects.keys().map(|id| (id.0, *id)).collect();
    let mut normal: Vec<(usize, u32)> = Vec::new();
    let mut packed: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (num, entry) in &doc.reference_table.entries {
        match entry {
            XrefEntry::Normal { offset, .. } => normal.push((*offset as usize, *num)),
            XrefEntry::Compressed { container, .. } => packed.entry(*container).or_default().push(*num),
            _ => {}
        }
    }
    normal.sort();

    let mut span = HashMap::new();
    for (i, (offset, num)) in normal.iter().enumerate() {
        let limit = normal.get(i + 1).map(|n| n.0).unwrap_or(bytes.len()).min(bytes.len());
        if *offset >= limit {
            continue;
        }
        let Some(id) = ids.get(num) else { continue };
        let end = object_end(bytes, *offset, limit, doc.objects.get(id));
        span.insert(*id, (end - offset) as u64);
    }
    for (container, members) in packed {
        let Some(cid) = ids.get(&container) else { continue };
        let total = span.get(cid).copied().unwrap_or(0);
        let lens: Vec<(ObjectId, u64)> = members
            .iter()
            .filter_map(|n| ids.get(n))
            .filter_map(|id| doc.objects.get(id).map(|o| (*id, approx_len(o) as u64)))
            .collect();
        let sum: u64 = lens.iter().map(|l| l.1).sum::<u64>().max(1);
        let mut given = 0;
        for (id, len) in lens {
            let share = total * len / sum;
            given += share;
            span.insert(id, share);
        }
        // Rounding leftovers stay with the container, which counts as overhead.
        span.insert(*cid, total - given.min(total));
    }
    span
}

/// End of the object starting at `start`: just past its `endobj`, or `limit`.
/// A stream's data is skipped by its known length, so binary data cannot fool the search.
fn object_end(bytes: &[u8], start: usize, limit: usize, obj: Option<&Object>) -> usize {
    let region = &bytes[start..limit];
    let from = match obj {
        Some(Object::Stream(s)) => find(region, b"stream")
            .map(|p| {
                let mut d = p + 6;
                if region.get(d) == Some(&b'\r') {
                    d += 1;
                }
                if region.get(d) == Some(&b'\n') {
                    d += 1;
                }
                (d + s.content.len()).min(region.len())
            })
            .unwrap_or(0),
        _ => 0,
    };
    match find(&region[from..], b"endobj") {
        Some(p) => start + from + p + 6,
        None => limit,
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Roughly how many bytes an object takes when written out.
fn approx_len(o: &Object) -> usize {
    match o {
        Object::Null => 4,
        Object::Boolean(_) => 5,
        Object::Integer(i) => i.to_string().len(),
        Object::Real(_) => 6,
        Object::Name(n) => n.len() + 1,
        Object::String(s, _) => s.len() + 2,
        Object::Array(a) => 2 + a.iter().map(|x| approx_len(x) + 1).sum::<usize>(),
        Object::Dictionary(d) => dict_len(d),
        Object::Stream(s) => dict_len(&s.dict) + s.content.len() + 20,
        Object::Reference(_) => 8,
    }
}

fn dict_len(d: &Dictionary) -> usize {
    4 + d.iter().map(|(k, v)| k.len() + 2 + approx_len(v)).sum::<usize>()
}

// ---- the object graph ----

fn children(o: &Object, out: &mut Vec<ObjectId>) {
    match o {
        Object::Reference(id) => out.push(*id),
        Object::Array(a) => a.iter().for_each(|x| children(x, out)),
        Object::Dictionary(d) => d.iter().for_each(|(_, v)| children(v, out)),
        Object::Stream(s) => s.dict.iter().for_each(|(_, v)| children(v, out)),
        _ => {}
    }
}

fn reachable(doc: &Document) -> HashSet<ObjectId> {
    let mut seen = HashSet::new();
    let mut queue = Vec::new();
    for (key, v) in doc.trailer.iter() {
        // A cross-reference stream's own dictionary doubles as the trailer; skip its bookkeeping.
        if matches!(key.as_slice(), b"Prev" | b"XRefStm") {
            continue;
        }
        children(v, &mut queue);
    }
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(o) = doc.objects.get(&id) {
            children(o, &mut queue);
        }
    }
    seen
}

fn type_of(d: &Dictionary) -> Option<&[u8]> {
    d.get(b"Type").and_then(|o| o.as_name()).ok()
}

fn subtype_of(d: &Dictionary) -> Option<&[u8]> {
    d.get(b"Subtype").and_then(|o| o.as_name()).ok()
}

fn is_image(d: &Dictionary) -> bool {
    subtype_of(d) == Some(b"Image")
}

/// Puts every object in one category. Three passes:
/// 1. A key says what it points to: `/Thumb`, `/PieceInfo`, `/Metadata`, a page's `/Contents`, ...
/// 2. An object's own `/Type` or `/Subtype` says what it is.
/// 3. Objects still unknown take the category of what refers to them, in a fixed order.
///
/// Anything unreachable from the trailer is `Unused`, whatever it looks like.
fn classify(doc: &Document, reachable: &HashSet<ObjectId>, pages: &BTreeMap<u32, ObjectId>) -> HashMap<ObjectId, Category> {
    use Category::*;
    let page_ids: HashSet<ObjectId> = pages.values().copied().collect();
    let mut cat: HashMap<ObjectId, Category> = HashMap::new();
    fn claim(val: &Object, c: Category, cat: &mut HashMap<ObjectId, Category>) {
        let mut ids = Vec::new();
        children(val, &mut ids);
        for id in ids {
            cat.entry(id).or_insert(c);
        }
    }
    // ICC profiles: [/ICCBased ref], at any depth of a colour space definition.
    fn icc(o: &Object, out: &mut Vec<ObjectId>) {
        match o {
            Object::Array(a) => {
                if a.first().and_then(|n| n.as_name().ok()) == Some(b"ICCBased") {
                    if let Some(Object::Reference(r)) = a.get(1) {
                        out.push(*r);
                    }
                }
                a.iter().for_each(|x| icc(x, out));
            }
            Object::Dictionary(d) => d.iter().for_each(|(_, v)| icc(v, out)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, v)| icc(v, out)),
            _ => {}
        }
    }

    // 1. Keys.
    for (id, obj) in &doc.objects {
        let dict = match obj {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        for (key, val) in dict.iter() {
            let c = match key.as_slice() {
                b"Thumb" => Thumbnails,
                b"PieceInfo" => EditorData,
                b"Metadata" => Metadata,
                b"AP" => Annotations,
                b"AcroForm" => Annotations,
                b"EF" | b"EmbeddedFiles" => Attachments,
                b"StructTreeRoot" => Tagging,
                b"Outlines" | b"Dests" => Navigation,
                b"DestOutputProfile" => ColourProfiles,
                b"Contents" if page_ids.contains(id) => PageContent,
                _ => continue,
            };
            claim(val, c, &mut cat);
        }
        let mut profiles = Vec::new();
        icc(obj, &mut profiles);
        for p in profiles {
            cat.entry(p).or_insert(ColourProfiles);
        }
    }
    if let Ok(Object::Reference(info)) = doc.trailer.get(b"Info") {
        cat.entry(*info).or_insert(Metadata);
    }

    // 2. Own type.
    for (id, obj) in &doc.objects {
        if cat.contains_key(id) {
            continue;
        }
        let (dict, is_stream) = match obj {
            Object::Dictionary(d) => (d, false),
            Object::Stream(s) => (&s.dict, true),
            _ => continue,
        };
        let t = type_of(dict);
        let st = subtype_of(dict);
        let c = match (t, st) {
            (_, Some(b"Image")) if is_stream => Images,
            (Some(b"Font" | b"FontDescriptor" | b"Encoding" | b"CMap"), _) => Fonts,
            (_, Some(b"Form")) if is_stream => VectorGraphics,
            (Some(b"Pattern" | b"ExtGState"), _) => VectorGraphics,
            _ if dict.has(b"ShadingType") || dict.has(b"PatternType") => VectorGraphics,
            (Some(b"Annot"), _) => Annotations,
            (Some(b"EmbeddedFile" | b"Filespec"), _) => Attachments,
            (Some(b"Metadata"), _) => Metadata,
            (Some(b"StructTreeRoot" | b"StructElem" | b"MCR" | b"OBJR"), _) => Tagging,
            (Some(b"Outlines"), _) => Navigation,
            _ if dict.has(b"Title") && dict.has(b"Parent") && (dict.has(b"Dest") || dict.has(b"A") || dict.has(b"First")) => {
                Navigation
            }
            (Some(b"XRef" | b"ObjStm"), _) => Overhead,
            (Some(b"Page" | b"Pages" | b"Catalog"), _) => Structure,
            _ => continue,
        };
        cat.insert(*id, c);
    }

    // 3. Inherit from whatever refers to them, without passing through other known objects.
    let order = [
        Fonts, Images, EditorData, Thumbnails, Metadata, Attachments, Annotations, VectorGraphics, PageContent, Tagging,
        Navigation, ColourProfiles, Structure,
    ];
    for c in order {
        let mut queue: Vec<ObjectId> = cat.iter().filter(|(_, v)| **v == c).map(|(k, _)| *k).collect();
        while let Some(id) = queue.pop() {
            let Some(obj) = doc.objects.get(&id) else { continue };
            let mut kids = Vec::new();
            children(obj, &mut kids);
            for k in kids {
                if doc.objects.contains_key(&k) && !cat.contains_key(&k) {
                    cat.insert(k, c);
                    queue.push(k);
                }
            }
        }
    }

    // Everything left that is in use is structure; anything unreachable is unused.
    for (id, obj) in &doc.objects {
        let bookkeeping = obj
            .as_dict()
            .ok()
            .or_else(|| obj.as_stream().ok().map(|s| &s.dict))
            .and_then(type_of)
            .map(|t| t == b"XRef" || t == b"ObjStm")
            .unwrap_or(false);
        if bookkeeping {
            cat.insert(*id, Overhead);
        } else if !reachable.contains(id) {
            cat.insert(*id, Unused);
        } else {
            cat.entry(*id).or_insert(Structure);
        }
    }
    // Ids claimed by a key but missing from the file are dropped.
    cat.retain(|id, _| doc.objects.contains_key(id));
    cat
}

// ---- small helpers ----

/// Fonts listed in a page's own or inherited resources.
fn resource_fonts(doc: &Document, page_id: ObjectId) -> Vec<ObjectId> {
    let mut out = Vec::new();
    let Ok((own, inherited)) = doc.get_page_resources(page_id) else { return out };
    let dicts = own.into_iter().chain(inherited.iter().filter_map(|id| doc.get_dictionary(*id).ok()));
    for res in dicts {
        if let Some(fonts) = res.get(b"Font").ok().and_then(|o| deref(doc, o).as_dict().ok()) {
            for (_, v) in fonts.iter() {
                if let Object::Reference(id) = v {
                    out.push(*id);
                }
            }
        }
    }
    out
}

fn deref<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    match obj {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(obj),
        _ => obj,
    }
}

fn name_of(o: Option<&Object>) -> Option<String> {
    o.and_then(|o| o.as_name().ok()).map(|n| String::from_utf8_lossy(n).into_owned())
}

/// `pdfaid:part` and `pdfaid:conformance` from XMP, as in `"2B"`.
fn pdfa_level(xmp: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(xmp);
    let value = |key: &str| -> Option<String> {
        let at = text.find(key)? + key.len();
        let rest = &text[at..];
        // Either an attribute (part="2") or an element (<pdfaid:part>2</pdfaid:part>).
        let rest = rest.trim_start_matches(['=', '"', '\'', '>', ' ']);
        let v: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        (!v.is_empty()).then_some(v)
    };
    let part = value("pdfaid:part")?;
    Some(format!("{part}{}", value("pdfaid:conformance").unwrap_or_default().to_uppercase()))
}

