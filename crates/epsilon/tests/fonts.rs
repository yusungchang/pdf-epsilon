//! Font subsetting on a synthetic TrueType font embedded the way Word and browsers do it:
//! Type 0, Identity-H, CIDFontType2, whole program in /FontFile2.

use epsilon::{compress_bytes, Preset};
use lopdf::{dictionary, Document, Object, Stream};

const GLYPHS: u16 = 12;

/// A TrueType font with 12 glyphs. Glyph g (1..) is a square of side 10·g; glyph 7 is a
/// composite made of glyph 9.
fn font() -> Vec<u8> {
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for g in 0..GLYPHS {
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        if g == 0 {
            continue;
        }
        let s = (g as i16) * 10;
        let mut glyph = Vec::new();
        if g == 7 {
            glyph.extend_from_slice(&(-1i16).to_be_bytes());
            glyph.extend_from_slice(&[0, 0, 0, 0, 0, 90, 0, 90]);
            // ARG_1_AND_2_ARE_WORDS | ARGS_ARE_XY_VALUES, glyph 9, offset (0, 0).
            glyph.extend_from_slice(&[0x00, 0x03, 0x00, 0x09, 0, 0, 0, 0]);
        } else {
            glyph.extend_from_slice(&1i16.to_be_bytes());
            for v in [0, 0, s, s] {
                glyph.extend_from_slice(&v.to_be_bytes());
            }
            glyph.extend_from_slice(&3u16.to_be_bytes()); // last point of the contour
            glyph.extend_from_slice(&0u16.to_be_bytes()); // no instructions
            glyph.extend_from_slice(&[0x01; 4]); // on-curve points, two-byte coordinates
            for v in [0, s, s, 0] {
                glyph.extend_from_slice(&v.to_be_bytes());
            }
            for v in [0, 0, s, s] {
                glyph.extend_from_slice(&v.to_be_bytes());
            }
        }
        glyf.extend_from_slice(&glyph);
        glyf.resize((glyf.len() + 3) & !3, 0);
    }
    loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());

    let mut head = vec![0u8; 54];
    head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[50..52].copy_from_slice(&1i16.to_be_bytes()); // long loca
    let mut maxp = 0x0000_5000u32.to_be_bytes().to_vec();
    maxp.extend_from_slice(&GLYPHS.to_be_bytes());
    let mut hhea = vec![0u8; 36];
    hhea[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    hhea[34..36].copy_from_slice(&GLYPHS.to_be_bytes());
    let hmtx: Vec<u8> = (0..GLYPHS).flat_map(|_| [0x01, 0xF4, 0, 0]).collect();

    let tables: Vec<(&[u8; 4], Vec<u8>)> =
        vec![(b"glyf", glyf), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp)];
    let mut out = vec![0, 1, 0, 0];
    out.extend_from_slice(&(tables.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 64, 0, 2, 0, 32]);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        body.resize((body.len() + 3) & !3, 0);
        offset = 12 + 16 * tables.len() + body.len();
    }
    out.extend_from_slice(&body);
    out
}

/// One page showing CIDs 3, 5 and 7. With `form_font`, the font is also in the form's
/// default resources, so it may be used for typing.
fn pdf(form_font: bool) -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let program = font();
    let mut file = Stream::new(dictionary! { "Length1" => program.len() as i64 }, program);
    file.compress().unwrap();
    let file_id = doc.add_object(file);
    let descriptor = doc.add_object(dictionary! {
        "Type" => "FontDescriptor", "FontName" => "Squares", "Flags" => 32,
        "FontBBox" => vec![0.into(), 0.into(), 1000.into(), 1000.into()], "ItalicAngle" => 0,
        "Ascent" => 1000, "Descent" => 0, "CapHeight" => 1000, "StemV" => 80, "FontFile2" => file_id,
    });
    let cid = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "Squares",
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "FontDescriptor" => descriptor, "CIDToGIDMap" => "Identity",
    });
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "Squares", "Encoding" => "Identity-H",
        "DescendantFonts" => vec![cid.into()],
    });
    let content = doc.add_object(Stream::new(dictionary! {}, b"BT /F1 12 Tf 72 700 Td <000300050007> Tj ET".to_vec()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    if form_font {
        catalog.set("AcroForm", dictionary! { "Fields" => vec![], "DR" => dictionary! { "Font" => dictionary! { "F1" => font_id } } });
    }
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// Glyph numbers with outlines, and the Type 0 font's name, in a saved PDF.
fn inspect(bytes: &[u8]) -> (Vec<u16>, Vec<u8>) {
    let doc = Document::load_mem(bytes).unwrap();
    let mut glyphs = Vec::new();
    let mut name = Vec::new();
    for obj in doc.objects.values() {
        if let Ok(d) = obj.as_dict() {
            if d.get(b"Subtype").and_then(|s| s.as_name()).ok() == Some(b"Type0") {
                name = d.get(b"BaseFont").unwrap().as_name().unwrap().to_vec();
            }
        }
        let Ok(s) = obj.as_stream() else { continue };
        if !s.dict.has(b"Length1") {
            continue;
        }
        let font = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
        let n = u16::from_be_bytes([font[4], font[5]]) as usize;
        let table = |tag: &[u8]| {
            (0..n).find_map(|i| {
                let r = &font[12 + 16 * i..28 + 16 * i];
                (&r[0..4] == tag).then(|| {
                    let off = u32::from_be_bytes(r[8..12].try_into().unwrap()) as usize;
                    let len = u32::from_be_bytes(r[12..16].try_into().unwrap()) as usize;
                    &font[off..off + len]
                })
            })
        };
        let loca = table(b"loca").unwrap();
        let at = |g: usize| u32::from_be_bytes(loca[g * 4..g * 4 + 4].try_into().unwrap());
        glyphs = (0..GLYPHS).filter(|g| at(*g as usize + 1) > at(*g as usize)).collect();
    }
    (glyphs, name)
}

#[test]
fn keeps_only_drawn_glyphs_and_their_components() {
    let input = pdf(false);
    let (before, _) = inspect(&input);
    assert_eq!(before, (1..GLYPHS).collect::<Vec<_>>());
    let (out, report) = compress_bytes(&input, &Preset::Lossless.options()).unwrap();
    assert_eq!(report.fonts_subset, 1);
    let (glyphs, name) = inspect(&out);
    // 3, 5 and 7 are drawn; 9 is part of 7. Glyph 0 is empty in this font.
    assert_eq!(glyphs, vec![3, 5, 7, 9]);
    assert!(name.len() == 14 && name[6] == b'+' && name.ends_with(b"Squares"), "{}", String::from_utf8_lossy(&name));
}

#[test]
fn fonts_for_form_fields_stay_whole() {
    let (out, report) = compress_bytes(&pdf(true), &Preset::Lossless.options()).unwrap();
    assert_eq!(report.fonts_subset, 0);
    let (glyphs, name) = inspect(&out);
    assert_eq!(glyphs, (1..GLYPHS).collect::<Vec<_>>());
    assert_eq!(name, b"Squares");
}
