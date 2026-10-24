//! Checks on every optimised copy, written apart from the code that changes the document so a
//! mistake there is not repeated here. Two kinds:
//!
//! - **Letters.** After fonts are subset, the document's text is walked again with its own
//!   reader of the graphics state (`q`/`Q`, `Tf`, fonts set by `gs`, fonts inherited by forms),
//!   and every glyph a page draws must still have its outline. If one is gone, the caller puts
//!   the fonts back whole. (In 0.6.1 a font chosen inside `q … Q` cost the outer font its
//!   glyphs, and no text comparison could see it.)
//! - **Structure.** The saved copy, read back from its bytes, must have the same number of
//!   annotations and usable resources on each page, and the same bookmarks, form fields,
//!   attachments and named destinations. A difference means something was lost, and the copy
//!   is refused.
//!
//! Every walk is bounded: a hostile file cannot make these checks run long.

use std::collections::{HashMap, HashSet};

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::limits;
use crate::placement::{deref, lookup, page_resources};

/// Most nodes any one count visits: outline items, fields, name-tree entries, forms.
const NODES: usize = 1_000_000;
/// Most drawing operations the letter check reads in a whole document.
const OPERATIONS: usize = 20_000_000;

// ---------- structure ----------

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Per page: annotations, and resource entries that point at something.
    pages: Vec<(usize, usize)>,
    outline: usize,
    fields: usize,
    files: usize,
    dests: usize,
}

/// What the structure check compares, counted on one document.
pub fn counts(doc: &Document) -> Counts {
    let catalog = doc.catalog().ok();
    fn get<'a>(doc: &'a Document, d: Option<&'a Dictionary>, k: &[u8]) -> Option<&'a Dictionary> {
        d.and_then(|d| d.get(k).ok()).and_then(|o| deref(doc, o).as_dict().ok())
    }
    let names = get(doc, catalog, b"Names");
    let mut forms = HashMap::new();
    let pages = doc
        .get_pages()
        .into_values()
        .map(|id| {
            let annots = doc
                .get_dictionary(id)
                .ok()
                .and_then(|p| p.get(b"Annots").ok())
                .and_then(|o| deref(doc, o).as_array().ok())
                .map(|a| a.iter().filter(|o| deref(doc, o).as_dict().is_ok()).count())
                .unwrap_or(0);
            let resources: usize = page_resources(doc, id).iter().map(|r| resource_entries(doc, r, &mut forms, 0)).fold(0, usize::saturating_add);
            (annots, resources)
        })
        .collect();
    Counts {
        pages,
        outline: get(doc, catalog, b"Outlines").map(|o| linked(doc, o, b"First")).unwrap_or(0),
        fields: get(doc, catalog, b"AcroForm")
            .and_then(|f| f.get(b"Fields").ok())
            .and_then(|o| deref(doc, o).as_array().ok())
            .map(|a| tree(doc, a, b"Kids"))
            .unwrap_or(0),
        files: get(doc, names, b"EmbeddedFiles").map(|t| name_tree(doc, t)).unwrap_or(0),
        dests: get(doc, names, b"Dests").map(|t| name_tree(doc, t)).unwrap_or(0) + get(doc, catalog, b"Dests").map(|d| d.len()).unwrap_or(0),
    }
}

/// Explains the first difference between two counts, or `Ok` when they match.
pub fn compare(before: &Counts, after: &Counts) -> Result<(), String> {
    if before.pages.len() != after.pages.len() {
        return Err(format!("{} pages instead of {}", after.pages.len(), before.pages.len()));
    }
    for (i, (b, a)) in before.pages.iter().zip(&after.pages).enumerate() {
        if b.0 != a.0 {
            return Err(format!("page {}: {} annotations instead of {}", i + 1, a.0, b.0));
        }
        if b.1 != a.1 {
            return Err(format!("page {}: {} resources instead of {}", i + 1, a.1, b.1));
        }
    }
    for (what, b, a) in [
        ("bookmarks", before.outline, after.outline),
        ("form fields", before.fields, after.fields),
        ("attachments", before.files, after.files),
        ("named destinations", before.dests, after.dests),
    ] {
        if b != a {
            return Err(format!("{a} {what} instead of {b}"));
        }
    }
    Ok(())
}

const KINDS: [&[u8]; 7] = [b"Font", b"XObject", b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"Properties"];

/// Resource entries that point at an object, including those of forms drawn from here.
/// Counted per entry, so two names for one object count twice; merging identical objects
/// does not change the total. A form's count is remembered by id, so a form drawn many times
/// costs one walk.
fn resource_entries(doc: &Document, res: &Dictionary, forms: &mut HashMap<ObjectId, usize>, depth: u32) -> usize {
    let mut n = 0usize;
    for kind in KINDS {
        let Some(entries) = res.get(kind).ok().and_then(|o| deref(doc, o).as_dict().ok()) else { continue };
        for (_, v) in entries.iter() {
            let present = match v {
                Object::Reference(id) => !matches!(doc.get_object(*id), Err(_) | Ok(Object::Null)),
                Object::Null => false,
                _ => true,
            };
            if !present {
                continue;
            }
            n = n.saturating_add(1);
            if kind == b"XObject" && depth < 16 {
                if let Object::Reference(id) = v {
                    n = n.saturating_add(form_entries(doc, *id, forms, depth));
                }
            }
        }
    }
    n
}

fn form_entries(doc: &Document, id: ObjectId, forms: &mut HashMap<ObjectId, usize>, depth: u32) -> usize {
    if let Some(n) = forms.get(&id) {
        return *n;
    }
    if forms.len() > NODES {
        return 0;
    }
    forms.insert(id, 0); // a form that draws itself counts once
    let Ok(Object::Stream(s)) = doc.get_object(id) else { return 0 };
    let n = s
        .dict
        .get(b"Resources")
        .ok()
        .and_then(|o| deref(doc, o).as_dict().ok())
        .filter(|_| matches!(s.dict.get(b"Subtype").and_then(|o| o.as_name()), Ok(b"Form")))
        .map(|r| resource_entries(doc, r, forms, depth + 1))
        .unwrap_or(0);
    forms.insert(id, n);
    n
}

/// Items in an outline: each item's children start at /First and continue through /Next.
fn linked(doc: &Document, parent: &Dictionary, first: &[u8]) -> usize {
    let mut n = 0;
    let mut seen = HashSet::new();
    let mut stack: Vec<ObjectId> = parent.get(first).ok().and_then(|o| o.as_reference().ok()).into_iter().collect();
    while let Some(id) = stack.pop() {
        if n >= NODES || !seen.insert(id) {
            continue;
        }
        let Ok(item) = doc.get_dictionary(id) else { continue };
        n += 1;
        for key in [&b"Next"[..], b"First"] {
            if let Ok(Object::Reference(next)) = item.get(key) {
                stack.push(*next);
            }
        }
    }
    n
}

/// Nodes in a tree of dictionaries linked by `kids`, such as form fields.
fn tree(doc: &Document, roots: &[Object], kids: &[u8]) -> usize {
    let mut n = 0;
    let mut seen = HashSet::new();
    let mut stack: Vec<&Object> = roots.iter().collect();
    while let Some(o) = stack.pop() {
        if n >= NODES {
            break;
        }
        if let Object::Reference(id) = o {
            if !seen.insert(*id) {
                continue;
            }
        }
        let Ok(d) = deref(doc, o).as_dict() else { continue };
        n += 1;
        if let Some(a) = d.get(kids).ok().and_then(|k| deref(doc, k).as_array().ok()) {
            stack.extend(a.iter());
        }
    }
    n
}

/// Entries in a name tree: pairs in /Names arrays, found through /Kids.
fn name_tree(doc: &Document, root: &Dictionary) -> usize {
    let mut n = 0;
    let mut nodes = 0;
    let mut seen = HashSet::new();
    let mut stack: Vec<&Dictionary> = vec![root];
    while let Some(d) = stack.pop() {
        nodes += 1;
        if nodes > NODES {
            break;
        }
        if let Some(a) = d.get(b"Names").ok().and_then(|o| deref(doc, o).as_array().ok()) {
            n += a.len() / 2;
        }
        if let Some(kids) = d.get(b"Kids").ok().and_then(|o| deref(doc, o).as_array().ok()) {
            for k in kids {
                if let Object::Reference(id) = k {
                    if !seen.insert(*id) {
                        continue;
                    }
                }
                if let Ok(kid) = deref(doc, k).as_dict() {
                    stack.push(kid);
                }
            }
        }
    }
    n
}

// ---------- letters ----------

/// Checks that every glyph drawn with a font whose program is in `originals` still has an
/// outline, unless it had none to begin with. `originals` holds those programs as they were
/// before subsetting, by object id, decoded.
pub fn glyphs_kept(doc: &Document, originals: &HashMap<ObjectId, Vec<u8>>) -> Result<(), String> {
    if originals.is_empty() {
        return Ok(());
    }
    let text = Text::collect(doc);
    for (font_id, strings) in &text.strings {
        let Some((program, map)) = cid_program(doc, *font_id) else { continue };
        let Some(before) = originals.get(&program) else { continue };
        let Some(had) = outlines(before) else { continue };
        let Some(now) = doc.get_object(program).ok().and_then(|o| o.as_stream().ok()).and_then(|s| limits::decode(s, limits::FONT)) else {
            return Err(format!("font program {} no longer decodes", program.0));
        };
        let Some(has) = outlines(&now) else { return Err(format!("font program {} no longer parses", program.0)) };
        for s in strings {
            for pair in s.chunks_exact(2) {
                let cid = u16::from_be_bytes([pair[0], pair[1]]);
                let gid = map.gid(cid) as usize;
                if had.get(gid) == Some(&true) && has.get(gid) != Some(&true) {
                    return Err(format!("glyph {gid} drawn with font {} was removed", font_id.0));
                }
            }
        }
    }
    Ok(())
}

enum Map {
    Identity,
    Table(Vec<u8>),
}

impl Map {
    fn gid(&self, cid: u16) -> u16 {
        match self {
            Map::Identity => cid,
            Map::Table(t) => t.get(cid as usize * 2..cid as usize * 2 + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0),
        }
    }
}

/// The TrueType program behind a Type 0 font with Identity encoding, and its CID-to-glyph map.
fn cid_program(doc: &Document, font: ObjectId) -> Option<(ObjectId, Map)> {
    let f = doc.get_dictionary(font).ok()?;
    if f.get(b"Subtype").ok()?.as_name().ok()? != b"Type0" {
        return None;
    }
    if !matches!(f.get(b"Encoding").ok()?.as_name().ok()?, b"Identity-H" | b"Identity-V") {
        return None;
    }
    let cid = deref(doc, deref(doc, f.get(b"DescendantFonts").ok()?).as_array().ok()?.first()?).as_dict().ok()?;
    let descriptor = deref(doc, cid.get(b"FontDescriptor").ok()?).as_dict().ok()?;
    let program = descriptor.get(b"FontFile2").ok()?.as_reference().ok()?;
    let map = match cid.get(b"CIDToGIDMap").ok().map(|o| deref(doc, o)) {
        None => Map::Identity,
        Some(Object::Name(n)) if n == b"Identity" => Map::Identity,
        Some(Object::Stream(s)) => Map::Table(limits::decode(s, limits::SMALL)?),
        _ => return None,
    };
    Some((program, map))
}

/// For each glyph of a TrueType font, whether it has an outline.
fn outlines(font: &[u8]) -> Option<Vec<bool>> {
    let (_, tables) = crate::fonts::parse(font)?;
    let get = |tag: &[u8; 4]| tables.iter().find(|t| &t.tag == tag).map(|t| t.data);
    let count = u16::from_be_bytes(get(b"maxp")?.get(4..6)?.try_into().ok()?) as usize;
    let long = i16::from_be_bytes(get(b"head")?.get(50..52)?.try_into().ok()?) == 1;
    let loca = get(b"loca")?;
    let at = |i: usize| -> Option<usize> {
        if long {
            loca.get(i * 4..i * 4 + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize)
        } else {
            loca.get(i * 2..i * 2 + 2).map(|b| u16::from_be_bytes(b.try_into().unwrap()) as usize * 2)
        }
    };
    (0..count).map(|g| Some(at(g + 1)? > at(g)?)).collect()
}

/// Text drawn with each font, found by reading content the way a viewer does.
#[derive(Default)]
struct Text {
    strings: HashMap<ObjectId, Vec<Vec<u8>>>,
    /// Forms already read with a given font in effect.
    seen: HashSet<(ObjectId, Option<ObjectId>)>,
    operations: usize,
}

impl Text {
    fn collect(doc: &Document) -> Text {
        let mut t = Text::default();
        for page in doc.get_pages().into_values() {
            let resources = page_resources(doc, page);
            if let Some(content) = limits::page_content(doc, page) {
                t.walk(doc, &content, &resources, None, 0);
            }
            // Annotation appearances start with no font selected.
            let annots = doc.get_dictionary(page).ok().and_then(|p| p.get(b"Annots").ok()).and_then(|o| deref(doc, o).as_array().ok());
            for a in annots.into_iter().flatten() {
                let Some(ap) = deref(doc, a).as_dict().ok().and_then(|a| a.get(b"AP").ok()).and_then(|o| deref(doc, o).as_dict().ok()) else { continue };
                for (_, v) in ap.iter() {
                    let mut streams = Vec::new();
                    match deref(doc, v) {
                        Object::Stream(_) => streams.extend(v.as_reference().ok()),
                        Object::Dictionary(states) => streams.extend(states.iter().filter_map(|(_, s)| s.as_reference().ok())),
                        _ => {}
                    }
                    for id in streams {
                        t.form(doc, id, &[], None, 0);
                    }
                }
            }
        }
        t
    }

    /// Reads a form or appearance stream with the font in effect where it is drawn.
    fn form(&mut self, doc: &Document, id: ObjectId, inherited: &[Dictionary], font: Option<ObjectId>, depth: u32) {
        if depth > 16 || !self.seen.insert((id, font)) {
            return;
        }
        let Ok(Object::Stream(s)) = doc.get_object(id) else { return };
        let mut resources = Vec::new();
        if let Some(own) = s.dict.get(b"Resources").ok().and_then(|o| deref(doc, o).as_dict().ok()) {
            resources.push(own.clone());
        }
        resources.extend_from_slice(inherited);
        if let Some(data) = limits::decode(s, limits::CONTENT) {
            self.walk(doc, &data, &resources, font, depth + 1);
        }
    }

    fn walk(&mut self, doc: &Document, data: &[u8], resources: &[Dictionary], mut font: Option<ObjectId>, depth: u32) {
        if self.operations > OPERATIONS || !limits::operations_ok(data) {
            return;
        }
        let Ok(content) = Content::decode(data) else { return };
        self.operations += content.operations.len();
        let mut saved = Vec::new();
        for op in &content.operations {
            let name = |i: usize| op.operands.get(i).and_then(|o| o.as_name().ok());
            match op.operator.as_str() {
                "q" => saved.push(font),
                "Q" => font = saved.pop().unwrap_or(font),
                "Tf" => font = name(0).and_then(|n| lookup(doc, resources, b"Font", n)),
                "gs" => {
                    // A graphics state's /Font is [font size].
                    let set = name(0)
                        .and_then(|n| lookup(doc, resources, b"ExtGState", n))
                        .and_then(|id| doc.get_dictionary(id).ok())
                        .and_then(|g| g.get(b"Font").ok())
                        .and_then(|o| deref(doc, o).as_array().ok())
                        .and_then(|a| a.first())
                        .and_then(|o| o.as_reference().ok());
                    if set.is_some() {
                        font = set;
                    }
                }
                "Tj" | "'" => self.add(font, op.operands.last()),
                "\"" => self.add(font, op.operands.get(2)),
                "TJ" => {
                    if let Some(Object::Array(parts)) = op.operands.first() {
                        for p in parts {
                            self.add(font, Some(p));
                        }
                    }
                }
                "Do" => {
                    let Some(id) = name(0).and_then(|n| lookup(doc, resources, b"XObject", n)) else { continue };
                    let is_form = matches!(doc.get_object(id), Ok(Object::Stream(s)) if matches!(s.dict.get(b"Subtype").and_then(|o| o.as_name()), Ok(b"Form")));
                    if is_form {
                        self.form(doc, id, resources, font, depth);
                    }
                }
                _ => {}
            }
        }
    }

    fn add(&mut self, font: Option<ObjectId>, s: Option<&Object>) {
        if let (Some(f), Some(Object::String(bytes, _))) = (font, s) {
            self.strings.entry(f).or_default().push(bytes.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};

    /// The tables the check reads (head, maxp, loca): glyph g has an outline when `drawn[g]`.
    fn font(drawn: &[bool]) -> Vec<u8> {
        let mut loca = Vec::new();
        let mut at = 0u32;
        for d in drawn {
            loca.extend_from_slice(&at.to_be_bytes());
            at += if *d { 12 } else { 0 };
        }
        loca.extend_from_slice(&at.to_be_bytes());
        let mut head = vec![0u8; 54];
        head[50..52].copy_from_slice(&1i16.to_be_bytes());
        let mut maxp = vec![0, 0, 0x50, 0];
        maxp.extend_from_slice(&(drawn.len() as u16).to_be_bytes());
        let tables = [(b"head", head), (b"loca", loca), (b"maxp", maxp)];
        let mut out = vec![0, 1, 0, 0, 0, tables.len() as u8, 0, 0, 0, 0, 0, 0];
        let mut body = Vec::new();
        for (tag, data) in &tables {
            out.extend_from_slice(*tag);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&((12 + 16 * tables.len() + body.len()) as u32).to_be_bytes());
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            body.extend_from_slice(data);
        }
        out.extend_from_slice(&body);
        out
    }

    /// F1 draws glyph 5 from inside a form, after F2 was chosen and dropped again by q … Q.
    fn doc(program: Vec<u8>) -> (Document, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages = doc.new_object_id();
        let file = doc.add_object(Stream::new(dictionary! {}, program));
        let mut type0 = |doc: &mut Document, file: ObjectId| {
            let descriptor = doc.add_object(dictionary! { "Type" => "FontDescriptor", "FontFile2" => file });
            let cid = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "CIDFontType2", "FontDescriptor" => descriptor });
            doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type0", "Encoding" => "Identity-H", "DescendantFonts" => vec![cid.into()] })
        };
        let f1 = type0(&mut doc, file);
        let other = doc.add_object(Stream::new(dictionary! {}, font(&[true; 8])));
        let f2 = type0(&mut doc, other);
        let form = doc.add_object(Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form" }, b"BT <0005> Tj ET".to_vec()));
        let content = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 9 Tf ET q BT /F2 9 Tf ET Q /Fm Do".to_vec()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages, "Contents" => content,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => f1, "F2" => f2 }, "XObject" => dictionary! { "Fm" => form } },
        });
        doc.objects.insert(pages, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        doc.trailer.set("Root", catalog);
        (doc, file)
    }

    #[test]
    fn finds_a_glyph_removed_from_the_font_that_draws_it() {
        let whole = font(&[true; 8]);
        let mut without_5 = [true; 8];
        without_5[5] = false;
        let (good, file) = doc(whole.clone());
        let originals = HashMap::from([(file, whole.clone())]);
        assert_eq!(glyphs_kept(&good, &originals), Ok(()));
        let (bad, file) = doc(font(&without_5));
        let originals = HashMap::from([(file, whole)]);
        assert!(glyphs_kept(&bad, &originals).unwrap_err().contains("glyph 5"));
    }

    #[test]
    fn counts_what_a_copy_must_keep() {
        let (d, _) = doc(font(&[true; 8]));
        let c = counts(&d);
        // Two fonts and one form on the page.
        assert_eq!(c.pages, vec![(0, 3)]);
        let mut fewer = counts(&d);
        fewer.pages[0].1 = 2;
        assert_eq!(compare(&c, &fewer), Err("page 1: 2 resources instead of 3".into()));
    }
}
