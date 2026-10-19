//! Subset fully embedded fonts to the glyphs the document draws.
//!
//! Handled: Type 0 fonts with a TrueType descendant (`CIDFontType2`) and `Identity-H` or
//! `Identity-V` encoding, which is how Word, browsers and most CJK documents embed fonts. There,
//! every two bytes of a text string are a CID, and the CID maps to a glyph number through
//! `/CIDToGIDMap`, so the glyphs in use can be read straight from the text.
//!
//! The outlines of glyphs not in use are emptied; glyph numbers stay the same, so no text,
//! width or ToUnicode entry changes. Glyphs that a composite glyph is built from are kept.
//!
//! Text is collected from everything that can draw it: page content, form XObjects, tiling
//! patterns, soft-mask groups, annotation appearances and Type 3 glyphs. A font is left whole
//! when it can be used for typing (it is in the form's default resources), when it is set
//! through a graphics state, when something that might draw it was not read, or when any of
//! its text is not whole two-byte codes. If any content fails to decode, nothing is subset.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::analyze::{children, reachable};
use crate::fonts::{self, Change, Table};
use crate::limits;
use crate::placement::{deref, lookup, page_resources};

/// Subsets every font this module can handle. Returns the font programs that shrank.
pub fn subset_fonts(doc: &mut Document) -> Vec<Change> {
    let Some(usage) = Usage::collect(doc) else { return Vec::new() };

    // Group the eligible font dictionaries by the program they embed: some producers point
    // many font dictionaries at one program, and all of their glyphs must stay.
    struct Program {
        fonts: Vec<(ObjectId, Option<ObjectId>, Option<ObjectId>)>,
        gids: BTreeSet<u16>,
        ok: bool,
    }
    let mut programs: HashMap<ObjectId, Program> = HashMap::new();
    for (&font_id, obj) in &doc.objects {
        let Ok(font) = obj.as_dict() else { continue };
        if !is_name(font.get(b"Type").ok(), b"Font") {
            continue;
        }
        let Some(target) = cid_truetype(doc, font) else { continue };
        let entry = programs.entry(target.program).or_insert(Program { fonts: Vec::new(), gids: BTreeSet::new(), ok: true });
        entry.fonts.push((font_id, target.descendant, target.descriptor));
        let usable = usage.safe(font_id) && target.map.is_some();
        match (usable, usage.text.get(&font_id)) {
            (true, Some(strings)) => {
                for s in strings {
                    if s.len() % 2 != 0 {
                        entry.ok = false;
                        break;
                    }
                    for pair in s.chunks(2) {
                        let cid = u16::from_be_bytes([pair[0], pair[1]]);
                        match target.map.as_ref().and_then(|m| m.gid(cid)) {
                            Some(gid) => {
                                entry.gids.insert(gid);
                            }
                            None => entry.ok = false,
                        }
                    }
                }
            }
            // Listed but never drawn: nothing to learn, nothing lost by keeping its glyphs.
            (true, None) => {}
            (false, _) => entry.ok = false,
        }
    }
    // Any other font dictionary that embeds the same program, but that this module does not
    // understand, keeps it whole.
    for obj in doc.objects.values() {
        let Ok(d) = obj.as_dict() else { continue };
        if !is_name(d.get(b"Type").ok(), b"FontDescriptor") {
            continue;
        }
        if let Ok(Object::Reference(p)) = d.get(b"FontFile2") {
            if let Some(entry) = programs.get_mut(p) {
                let claimed = entry.fonts.iter().any(|(_, _, desc)| {
                    desc.and_then(|id| doc.get_dictionary(id).ok()).map(|x| std::ptr::eq(x, d)).unwrap_or(false)
                });
                if !claimed && !inline_descriptor_claimed(doc, &entry.fonts, d) {
                    entry.ok = false;
                }
            }
        }
    }

    let mut changes = Vec::new();
    let mut ids: Vec<ObjectId> = programs.keys().copied().collect();
    ids.sort();
    for program_id in ids {
        let p = &programs[&program_id];
        if !p.ok || p.gids.is_empty() {
            continue;
        }
        let Ok(Object::Stream(stream)) = doc.get_object(program_id) else { continue };
        let Some(data) = limits::decode(stream, limits::FONT) else { continue };
        let Some(font) = subset_glyphs(&data, &p.gids) else { continue };
        let Some(encoded) = fonts::deflate(&font) else { continue };
        let before = stream.content.len();
        if encoded.len() >= before {
            continue;
        }
        let mut dict = stream.dict.clone();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        dict.remove(b"DecodeParms");
        dict.set("Length1", Object::Integer(font.len() as i64));
        let mut new_stream = Stream::new(dict, encoded);
        new_stream.allows_compression = false;
        let after = new_stream.content.len();
        doc.objects.insert(program_id, Object::Stream(new_stream));

        // ISO 32000-1 9.6.4: a subset font's name starts with a six-letter tag.
        let tag = subset_tag(&p.gids);
        let fonts = p.fonts.clone();
        for (font_id, descendant, descriptor) in fonts {
            tag_name(doc, font_id, b"BaseFont", &tag);
            if let Some(id) = descendant {
                tag_name(doc, id, b"BaseFont", &tag);
            }
            if let Some(id) = descriptor {
                tag_name(doc, id, b"FontName", &tag);
            }
        }
        changes.push(Change { id: program_id, before, after });
    }
    changes
}

/// Descriptors written inline in the descendant font have no id; match them by content.
fn inline_descriptor_claimed(doc: &Document, fonts: &[(ObjectId, Option<ObjectId>, Option<ObjectId>)], d: &Dictionary) -> bool {
    fonts.iter().any(|(font_id, descendant, _)| {
        let cid = descendant.and_then(|id| doc.get_dictionary(id).ok()).or_else(|| {
            doc.get_dictionary(*font_id)
                .ok()
                .and_then(|f| f.get(b"DescendantFonts").ok())
                .and_then(|o| deref(doc, o).as_array().ok())
                .and_then(|a| a.first())
                .and_then(|o| o.as_dict().ok())
        });
        cid.and_then(|c| c.get(b"FontDescriptor").ok()).map(|o| matches!(o, Object::Dictionary(x) if x == d)).unwrap_or(false)
    })
}

struct Target {
    program: ObjectId,
    descendant: Option<ObjectId>,
    descriptor: Option<ObjectId>,
    /// `None` when the CID-to-glyph map is something this module does not read.
    map: Option<GidMap>,
}

enum GidMap {
    Identity,
    Table(Vec<u8>),
}

impl GidMap {
    fn gid(&self, cid: u16) -> Option<u16> {
        match self {
            GidMap::Identity => Some(cid),
            GidMap::Table(t) => {
                let i = cid as usize * 2;
                // CIDs past the end of the map have no glyph; they draw .notdef.
                Some(t.get(i..i + 2).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0))
            }
        }
    }
}

/// A Type 0 font with Identity encoding over an embedded TrueType CIDFont.
fn cid_truetype(doc: &Document, font: &Dictionary) -> Option<Target> {
    if !is_name(font.get(b"Subtype").ok(), b"Type0") {
        return None;
    }
    let encoding = font.get(b"Encoding").ok()?.as_name().ok()?;
    if encoding != b"Identity-H" && encoding != b"Identity-V" {
        return None;
    }
    let first = deref(doc, font.get(b"DescendantFonts").ok()?).as_array().ok()?.first()?;
    let descendant = first.as_reference().ok();
    let cid = deref(doc, first).as_dict().ok()?;
    if !is_name(cid.get(b"Subtype").ok(), b"CIDFontType2") {
        return None;
    }
    let descriptor_obj = cid.get(b"FontDescriptor").ok()?;
    let descriptor = descriptor_obj.as_reference().ok();
    let program = deref(doc, descriptor_obj).as_dict().ok()?.get(b"FontFile2").ok()?.as_reference().ok()?;
    let map = match cid.get(b"CIDToGIDMap").ok().map(|o| deref(doc, o)) {
        None => Some(GidMap::Identity),
        Some(Object::Name(n)) if n == b"Identity" => Some(GidMap::Identity),
        Some(Object::Stream(s)) => limits::decode(s, limits::SMALL).map(GidMap::Table),
        _ => None,
    };
    Some(Target { program, descendant, descriptor, map })
}

// ---- where text is drawn ----

#[derive(Default)]
struct Usage {
    /// Strings shown with each font.
    text: HashMap<ObjectId, Vec<Vec<u8>>>,
    /// Fonts that must stay whole.
    unsafe_fonts: HashSet<ObjectId>,
    /// Content streams read so far.
    walked: HashSet<ObjectId>,
    /// Nothing can be subset: some content could not be read, or might draw any font.
    give_up: bool,
}

impl Usage {
    fn collect(doc: &Document) -> Option<Usage> {
        let mut u = Usage::default();
        for page_id in doc.get_pages().into_values() {
            let resources = page_resources(doc, page_id);
            match limits::page_content(doc, page_id) {
                Some(content) => u.walk(doc, &content, &resources, 0),
                None => u.give_up = true,
            }
            // Annotation appearances: /AP holds streams, or dictionaries of streams by state.
            let annots = doc
                .get_dictionary(page_id)
                .ok()
                .and_then(|p| p.get(b"Annots").ok())
                .and_then(|o| deref(doc, o).as_array().ok())
                .cloned()
                .unwrap_or_default();
            for annot in annots {
                let Ok(a) = deref(doc, &annot).as_dict() else { continue };
                let Some(ap) = a.get(b"AP").ok().and_then(|o| deref(doc, o).as_dict().ok()) else { continue };
                for (_, v) in ap.iter() {
                    match v {
                        Object::Reference(id) => match doc.get_object(*id) {
                            Ok(Object::Stream(_)) => u.walk_stream(doc, *id, &[], 0),
                            Ok(Object::Dictionary(states)) => {
                                for (_, s) in states.iter() {
                                    if let Object::Reference(sid) = s {
                                        u.walk_stream(doc, *sid, &[], 0);
                                    }
                                }
                            }
                            _ => {}
                        },
                        Object::Dictionary(states) => {
                            for (_, s) in states.iter() {
                                if let Object::Reference(sid) = s {
                                    u.walk_stream(doc, *sid, &[], 0);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // Fonts available for typing into form fields.
        if let Some(fonts) = doc
            .catalog()
            .ok()
            .and_then(|c| c.get(b"AcroForm").ok())
            .and_then(|o| deref(doc, o).as_dict().ok())
            .and_then(|f| f.get(b"DR").ok())
            .and_then(|o| deref(doc, o).as_dict().ok())
            .and_then(|r| r.get(b"Font").ok())
            .and_then(|o| deref(doc, o).as_dict().ok())
        {
            for (_, v) in fonts.iter() {
                if let Object::Reference(id) = v {
                    u.unsafe_fonts.insert(*id);
                }
            }
        }

        // Anything in use that can draw text but was not read keeps its fonts whole. One with no
        // resources of its own could draw any font, so then nothing is subset.
        let live = reachable(doc);
        for (id, obj) in &doc.objects {
            let Object::Stream(s) = obj else { continue };
            if u.walked.contains(id) || !live.contains(id) || !draws(&s.dict) {
                continue;
            }
            match s.dict.get(b"Resources").ok().and_then(|o| deref(doc, o).as_dict().ok()) {
                Some(res) => u.mark_fonts(doc, res),
                None => u.give_up = true,
            }
        }
        // A graphics state can set a font with /Font; those fonts stay whole.
        for obj in doc.objects.values() {
            if let Ok(d) = obj.as_dict() {
                if is_name(d.get(b"Type").ok(), b"ExtGState") || d.has(b"Font") && !d.has(b"Type") {
                    if let Ok(f) = d.get(b"Font") {
                        let mut ids = Vec::new();
                        children(f, &mut ids);
                        u.unsafe_fonts.extend(ids);
                    }
                }
            }
        }
        (!u.give_up).then_some(u)
    }

    fn safe(&self, font: ObjectId) -> bool {
        !self.unsafe_fonts.contains(&font)
    }

    fn mark_fonts(&mut self, doc: &Document, resources: &Dictionary) {
        if let Some(fonts) = resources.get(b"Font").ok().and_then(|o| deref(doc, o).as_dict().ok()) {
            for (_, v) in fonts.iter() {
                if let Object::Reference(id) = v {
                    self.unsafe_fonts.insert(*id);
                }
            }
        }
    }

    /// Walks a form-like stream with its own resources, falling back to the caller's.
    fn walk_stream(&mut self, doc: &Document, id: ObjectId, inherited: &[Dictionary], depth: u32) {
        if depth > 16 || !self.walked.insert(id) {
            return;
        }
        let Ok(Object::Stream(s)) = doc.get_object(id) else { return };
        let mut resources = Vec::new();
        if let Some(own) = s.dict.get(b"Resources").ok().and_then(|o| deref(doc, o).as_dict().ok()) {
            resources.push(own.clone());
        }
        resources.extend_from_slice(inherited);
        match limits::decode(s, limits::CONTENT) {
            Some(data) => self.walk(doc, &data, &resources, depth),
            None => self.give_up = true,
        }
    }

    fn walk(&mut self, doc: &Document, content: &[u8], resources: &[Dictionary], depth: u32) {
        if depth > 16 {
            self.give_up = true;
            return;
        }
        if !limits::operations_ok(content) {
            self.give_up = true;
            return;
        }
        let Ok(content) = Content::decode(content) else {
            self.give_up = true;
            return;
        };
        let mut font: Option<ObjectId> = None;
        for op in &content.operations {
            let strings: Vec<&[u8]> = match op.operator.as_str() {
                "Tf" => {
                    font = op.operands.first().and_then(|o| o.as_name().ok()).and_then(|n| lookup(doc, resources, b"Font", n));
                    if let Some(id) = font {
                        self.type3(doc, id, resources, depth);
                    }
                    continue;
                }
                "Tj" | "'" => op.operands.last().and_then(|o| o.as_str().ok()).into_iter().collect(),
                "\"" => op.operands.get(2).and_then(|o| o.as_str().ok()).into_iter().collect(),
                "TJ" => op
                    .operands
                    .first()
                    .and_then(|o| o.as_array().ok())
                    .map(|a| a.iter().filter_map(|x| x.as_str().ok()).collect())
                    .unwrap_or_default(),
                "Do" => {
                    if let Some(id) = op.operands.first().and_then(|o| o.as_name().ok()).and_then(|n| lookup(doc, resources, b"XObject", n)) {
                        if let Ok(Object::Stream(s)) = doc.get_object(id) {
                            if is_name(s.dict.get(b"Subtype").ok(), b"Form") {
                                self.walk_stream(doc, id, resources, depth + 1);
                            }
                        }
                    }
                    continue;
                }
                "scn" | "SCN" => {
                    if let Some(id) = op.operands.last().and_then(|o| o.as_name().ok()).and_then(|n| lookup(doc, resources, b"Pattern", n)) {
                        if matches!(doc.get_object(id), Ok(Object::Stream(_))) {
                            self.walk_stream(doc, id, resources, depth + 1);
                        }
                    }
                    continue;
                }
                "gs" => {
                    // A soft mask's group is drawn content too.
                    let group = op
                        .operands
                        .first()
                        .and_then(|o| o.as_name().ok())
                        .and_then(|n| lookup(doc, resources, b"ExtGState", n))
                        .and_then(|id| doc.get_dictionary(id).ok())
                        .and_then(|gs| gs.get(b"SMask").ok())
                        .and_then(|o| deref(doc, o).as_dict().ok())
                        .and_then(|m| m.get(b"G").ok())
                        .and_then(|o| o.as_reference().ok());
                    if let Some(id) = group {
                        self.walk_stream(doc, id, resources, depth + 1);
                    }
                    continue;
                }
                _ => continue,
            };
            match font {
                Some(id) => self.text.entry(id).or_default().extend(strings.into_iter().map(<[u8]>::to_vec)),
                // Text with no font selected cannot be attributed; be safe.
                None if !strings.is_empty() => self.give_up = true,
                None => {}
            }
        }
    }

    /// A Type 3 font's glyphs are content streams that can draw other fonts.
    fn type3(&mut self, doc: &Document, font: ObjectId, resources: &[Dictionary], depth: u32) {
        let Ok(f) = doc.get_dictionary(font) else { return };
        if !is_name(f.get(b"Subtype").ok(), b"Type3") {
            return;
        }
        let mut res = Vec::new();
        if let Some(own) = f.get(b"Resources").ok().and_then(|o| deref(doc, o).as_dict().ok()) {
            res.push(own.clone());
        }
        res.extend_from_slice(resources);
        let procs: Vec<ObjectId> = f
            .get(b"CharProcs")
            .ok()
            .and_then(|o| deref(doc, o).as_dict().ok())
            .map(|d| d.iter().filter_map(|(_, v)| v.as_reference().ok()).collect())
            .unwrap_or_default();
        for id in procs {
            self.walk_stream(doc, id, &res, depth + 1);
        }
    }
}

/// Streams that hold drawing instructions: form XObjects and tiling patterns.
/// (Page contents, appearances and Type 3 glyphs are reached from their owners.)
fn draws(dict: &Dictionary) -> bool {
    is_name(dict.get(b"Subtype").ok(), b"Form") || matches!(dict.get(b"PatternType").and_then(|o| o.as_i64()), Ok(1))
}

fn is_name(o: Option<&Object>, name: &[u8]) -> bool {
    matches!(o.and_then(|o| o.as_name().ok()), Some(n) if n == name)
}

// ---- the font program ----

/// The font with every glyph outside `keep` (and the components they need) emptied, or `None`
/// when the font is not a TrueType font this understands.
fn subset_glyphs(font: &[u8], keep: &BTreeSet<u16>) -> Option<Vec<u8>> {
    let (version, tables) = fonts::parse(font)?;
    if &version == b"OTTO" {
        return None;
    }
    let get = |tag: &[u8; 4]| tables.iter().find(|t| &t.tag == tag).map(|t| t.data);
    let head = get(b"head")?;
    let glyf = get(b"glyf")?;
    let loca = get(b"loca")?;
    let count = u16::from_be_bytes(get(b"maxp")?.get(4..6)?.try_into().ok()?) as usize;
    let long = i16::from_be_bytes(head.get(50..52)?.try_into().ok()?) == 1;
    let offsets: Vec<usize> = (0..=count)
        .map(|i| {
            if long {
                loca.get(i * 4..i * 4 + 4).map(|b| u32::from_be_bytes(b.try_into().unwrap()) as usize)
            } else {
                loca.get(i * 2..i * 2 + 2).map(|b| u16::from_be_bytes(b.try_into().unwrap()) as usize * 2)
            }
        })
        .collect::<Option<_>>()?;
    let glyph = |g: usize| -> Option<&[u8]> {
        let (a, b) = (offsets[g], offsets[g + 1]);
        if b < a {
            return None;
        }
        glyf.get(a..b)
    };

    // Glyph 0 (.notdef) always stays; composites pull in their components.
    let mut kept: BTreeSet<usize> = keep.iter().map(|g| *g as usize).filter(|g| *g < count).collect();
    kept.insert(0);
    let mut queue: Vec<usize> = kept.iter().copied().collect();
    while let Some(g) = queue.pop() {
        for c in components(glyph(g)?)? {
            if c < count && kept.insert(c) {
                queue.push(c);
            }
        }
    }
    if kept.len() * 10 > count * 9 {
        return None; // Almost everything is in use; not worth rewriting.
    }

    let mut new_glyf = Vec::new();
    let mut new_loca = Vec::with_capacity((count + 1) * 4);
    for g in 0..count {
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        if kept.contains(&g) {
            new_glyf.extend_from_slice(glyph(g)?);
            new_glyf.resize((new_glyf.len() + 3) & !3, 0);
        }
    }
    new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
    let mut new_head = head.to_vec();
    new_head[50..52].copy_from_slice(&1i16.to_be_bytes()); // long loca offsets

    let mut out: Vec<Table> = Vec::with_capacity(tables.len());
    for t in &tables {
        let data: &[u8] = match &t.tag {
            b"glyf" => &new_glyf,
            b"loca" => &new_loca,
            b"head" => &new_head,
            // Device metrics and hint caches describe every glyph and are not read by viewers.
            b"hdmx" | b"LTSH" | b"VDMX" | b"DSIG" => continue,
            _ => t.data,
        };
        out.push(Table { tag: t.tag, data });
    }
    Some(fonts::write(&version, out.iter().collect()))
}

/// Glyph numbers a composite glyph is built from; empty for a simple glyph.
fn components(glyph: &[u8]) -> Option<Vec<usize>> {
    if glyph.len() < 10 || i16::from_be_bytes([glyph[0], glyph[1]]) >= 0 {
        return Some(Vec::new());
    }
    const WORDS: u16 = 0x0001;
    const SCALE: u16 = 0x0008;
    const MORE: u16 = 0x0020;
    const XY_SCALE: u16 = 0x0040;
    const TWO_BY_TWO: u16 = 0x0080;
    let mut out = Vec::new();
    let mut at = 10;
    loop {
        let flags = u16::from_be_bytes(glyph.get(at..at + 2)?.try_into().ok()?);
        out.push(u16::from_be_bytes(glyph.get(at + 2..at + 4)?.try_into().ok()?) as usize);
        at += 4 + if flags & WORDS != 0 { 4 } else { 2 };
        at += if flags & SCALE != 0 {
            2
        } else if flags & XY_SCALE != 0 {
            4
        } else if flags & TWO_BY_TWO != 0 {
            8
        } else {
            0
        };
        if flags & MORE == 0 {
            return Some(out);
        }
    }
}

/// Six capital letters derived from the glyph set, as subset tags are.
fn subset_tag(gids: &BTreeSet<u16>) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    gids.hash(&mut h);
    let mut n = h.finish();
    (0..6)
        .map(|_| {
            let c = (b'A' + (n % 26) as u8) as char;
            n /= 26;
            c
        })
        .collect()
}

/// Prefixes a font name with a subset tag, unless it already has one.
fn tag_name(doc: &mut Document, id: ObjectId, key: &[u8], tag: &str) {
    let Ok(d) = doc.get_dictionary_mut(id) else { return };
    let Ok(Object::Name(name)) = d.get(key) else { return };
    let tagged = name.len() > 7 && name[6] == b'+' && name[..6].iter().all(|c| c.is_ascii_uppercase());
    if !tagged {
        let mut new = format!("{tag}+").into_bytes();
        new.extend_from_slice(name);
        d.set(key, Object::Name(new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_components_are_found() {
        // numberOfContours -1, bbox, then two components: words + MORE, then bytes + SCALE.
        let mut g = vec![0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0];
        g.extend_from_slice(&[0x00, 0x21, 0x00, 0x09, 0, 0, 0, 0]);
        g.extend_from_slice(&[0x00, 0x08, 0x00, 0x04, 0, 0, 0x40, 0x00]);
        assert_eq!(components(&g), Some(vec![9, 4]));
        assert_eq!(components(&[0, 1, 0, 0, 0, 0, 0, 0, 0, 0]), Some(vec![]));
    }

    #[test]
    fn tags_are_six_capitals() {
        let t = subset_tag(&[1u16, 2, 3].into_iter().collect());
        assert_eq!(t.len(), 6);
        assert!(t.bytes().all(|c| c.is_ascii_uppercase()));
    }
}
