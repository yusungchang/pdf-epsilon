//! Remove data from embedded fonts that no PDF viewer reads.
//!
//! An embedded TrueType or OpenType font (`/FontFile2`, or `/FontFile3` with `/Subtype
//! /OpenType`) is a table directory. A PDF viewer draws glyphs from their outlines and takes
//! positions from the page, so it needs only a few tables: ISO 32000-1 9.9 lists head, hhea,
//! loca, maxp, cvt, prep, glyf, hmtx and fpgm, plus cmap for simple fonts. Fonts saved by some
//! applications carry much more. macOS Quartz, for one, embeds CJK fonts with their screen
//! bitmaps (`bdat`/`bloc`), several megabytes each, and Apple layout tables.
//!
//! Tables are kept by an allow list, not dropped by a deny list, so an unknown table is removed
//! only because the list does not name it. The list keeps more than the minimum (cmap, OS/2,
//! name, post, vertical metrics, colour glyphs) so viewers that consult them still can.
//! Outlines and glyph numbers never change, so this is lossless. A font is left alone when it
//! has no outlines (a bitmap-only font), when it is a collection, or when anything about its
//! table directory does not add up.

use std::io::Write;

use lopdf::{Document, Object, ObjectId, Stream};

/// Tables a PDF viewer may use. Everything else is dropped.
const KEEP: &[&[u8; 4]] = &[
    // Required by ISO 32000-1 9.9 for TrueType.
    b"head", b"hhea", b"hmtx", b"loca", b"maxp", b"glyf", b"cvt ", b"fpgm", b"prep",
    // Glyph lookup for simple fonts, by code or by glyph name.
    b"cmap", b"post",
    // Small, and consulted by some viewers for metrics, names and hinting behaviour.
    b"OS/2", b"name", b"gasp",
    // Vertical writing.
    b"vhea", b"vmtx", b"VORG",
    // CFF outlines in an OpenType wrapper.
    b"CFF ", b"CFF2",
    // Colour glyphs: for some glyphs these are the only drawing there is.
    b"COLR", b"CPAL", b"sbix", b"CBDT", b"CBLC", b"SVG ",
];

pub struct Change {
    pub id: ObjectId,
    /// Encoded size of the font stream before and after.
    pub before: usize,
    pub after: usize,
}

/// Rewrites every embedded sfnt font that has tables to drop. Returns what changed.
pub fn strip_tables(doc: &mut Document) -> Vec<Change> {
    let mut programs: Vec<(ObjectId, bool)> = Vec::new();
    for obj in doc.objects.values() {
        let Ok(d) = obj.as_dict() else { continue };
        if !matches!(d.get(b"Type").and_then(|t| t.as_name()), Ok(b"FontDescriptor")) {
            continue;
        }
        if let Ok(Object::Reference(id)) = d.get(b"FontFile2") {
            programs.push((*id, true));
        }
        if let Ok(Object::Reference(id)) = d.get(b"FontFile3") {
            programs.push((*id, false));
        }
    }
    programs.sort();
    programs.dedup();

    let mut changes = Vec::new();
    for (id, truetype) in programs {
        let Ok(Object::Stream(stream)) = doc.get_object(id) else { continue };
        // FontFile3 holds other formats too (bare CFF); only the OpenType wrapper is an sfnt.
        if !truetype && !matches!(stream.dict.get(b"Subtype").and_then(|s| s.as_name()), Ok(b"OpenType")) {
            continue;
        }
        let Ok(data) = stream.decompressed_content().or_else(|_| {
            if stream.dict.has(b"Filter") { Err(()) } else { Ok(stream.content.clone()) }
        }) else {
            continue;
        };
        let Some(font) = rebuild(&data) else { continue };
        let Some(encoded) = deflate(&font) else { continue };
        let before = stream.content.len();
        if encoded.len() >= before {
            continue;
        }
        let mut dict = stream.dict.clone();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        dict.remove(b"DecodeParms");
        if truetype {
            // Length1 is the decoded length of a TrueType program.
            dict.set("Length1", Object::Integer(font.len() as i64));
        }
        let mut new_stream = Stream::new(dict, encoded);
        new_stream.allows_compression = false;
        let after = new_stream.content.len();
        doc.objects.insert(id, Object::Stream(new_stream));
        changes.push(Change { id, before, after });
    }
    changes
}

pub(crate) struct Table<'a> {
    pub tag: [u8; 4],
    pub data: &'a [u8],
}

/// The sfnt version and tables of a TrueType or OpenType font, or `None` for anything else,
/// including collections.
pub(crate) fn parse(font: &[u8]) -> Option<([u8; 4], Vec<Table<'_>>)> {
    let version: [u8; 4] = font.get(0..4)?.try_into().ok()?;
    if !matches!(&version, [0, 1, 0, 0] | b"true" | b"OTTO") {
        return None;
    }
    let count = u16::from_be_bytes(font.get(4..6)?.try_into().ok()?) as usize;
    let mut tables = Vec::with_capacity(count);
    for i in 0..count {
        let rec = font.get(12 + 16 * i..28 + 16 * i)?;
        let tag: [u8; 4] = rec[0..4].try_into().ok()?;
        let offset = u32::from_be_bytes(rec[8..12].try_into().ok()?) as usize;
        let length = u32::from_be_bytes(rec[12..16].try_into().ok()?) as usize;
        let data = font.get(offset..offset.checked_add(length)?)?;
        tables.push(Table { tag, data });
    }
    Some((version, tables))
}

/// The font with only the kept tables, or `None` when there is nothing to drop or the font
/// is not one this module understands.
fn rebuild(font: &[u8]) -> Option<Vec<u8>> {
    // TrueType (1.0 or 'true') or OpenType with CFF ('OTTO'). Collections ('ttcf') are left alone.
    let (version, tables) = parse(font)?;

    let has = |tag: &[u8; 4]| tables.iter().any(|t| &t.tag == tag && !t.data.is_empty());
    // Outlines must be there, or the bitmaps may be the only drawing the font has.
    let outlines = (has(b"glyf") && has(b"loca")) || has(b"CFF ") || has(b"CFF2");
    if !outlines || !has(b"head") || !has(b"maxp") {
        return None;
    }
    let kept: Vec<&Table> = tables.iter().filter(|t| KEEP.contains(&&t.tag)).collect();
    if kept.len() == tables.len() {
        return None;
    }
    Some(write(&version, kept))
}

/// Writes a table directory and its tables, aligned to four bytes, with fresh checksums.
pub(crate) fn write(version: &[u8], mut tables: Vec<&Table>) -> Vec<u8> {
    tables.sort_by_key(|t| t.tag);
    let n = tables.len() as u16;
    let entry_selector = 15 - n.max(1).leading_zeros() as u16;
    let search_range = (1u16 << entry_selector) * 16;
    let mut out = Vec::new();
    out.extend_from_slice(version);
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&(n * 16 - search_range).to_be_bytes());

    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    let mut body = Vec::new();
    for t in &tables {
        let mut data = t.data.to_vec();
        if &t.tag == b"head" && data.len() >= 12 {
            // checkSumAdjustment is computed over the whole font below.
            data[8..12].copy_from_slice(&[0; 4]);
            head_at = Some(offset);
        }
        out.extend_from_slice(&t.tag);
        out.extend_from_slice(&checksum(&data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let padded = (data.len() + 3) & !3;
        body.extend_from_slice(&data);
        body.resize(body.len() + padded - data.len(), 0);
        offset += padded;
    }
    out.extend_from_slice(&body);
    if let Some(at) = head_at {
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[at + 8..at + 12].copy_from_slice(&adjustment.to_be_bytes());
    }
    out
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut word = [0u8; 4];
        word[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

pub(crate) fn deflate(data: &[u8]) -> Option<Vec<u8>> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    enc.write_all(data).ok()?;
    enc.finish().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font(tags: &[&[u8; 4]]) -> Vec<u8> {
        let tables: Vec<Table> = tags
            .iter()
            .map(|t| Table { tag: **t, data: if *t == b"head" { &[0u8; 54][..] } else { &b"data"[..] } })
            .collect();
        write(&[0, 1, 0, 0], tables.iter().collect())
    }

    fn tags(font: &[u8]) -> Vec<String> {
        let n = u16::from_be_bytes([font[4], font[5]]) as usize;
        (0..n).map(|i| String::from_utf8_lossy(&font[12 + 16 * i..16 + 16 * i]).into_owned()).collect()
    }

    #[test]
    fn drops_bitmaps_and_apple_tables_keeps_outlines() {
        let input = font(&[b"head", b"maxp", b"glyf", b"loca", b"hmtx", b"bdat", b"bloc", b"Zapf", b"cmap"]);
        let out = rebuild(&input).unwrap();
        assert_eq!(tags(&out), ["cmap", "glyf", "head", "hmtx", "loca", "maxp"]);
        // A valid font sums to the magic number once checkSumAdjustment is set.
        assert_eq!(checksum(&out), 0xB1B0_AFBA);
    }

    #[test]
    fn leaves_bitmap_only_and_tidy_fonts_alone() {
        assert!(rebuild(&font(&[b"head", b"maxp", b"bdat", b"bloc"])).is_none(), "no outlines");
        assert!(rebuild(&font(&[b"head", b"maxp", b"glyf", b"loca", b"hmtx"])).is_none(), "nothing to drop");
        assert!(rebuild(b"ttcf\0\0\0\0").is_none(), "collection");
        assert!(rebuild(b"junk").is_none());
    }
}
