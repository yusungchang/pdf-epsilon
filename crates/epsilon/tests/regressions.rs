//! Content-loss bugs found in the review before 1.0 (see docs/RELEASE-1.0.md). Each test builds
//! the smallest PDF that showed the bug and checks the output still shows the same thing.

use epsilon::{compress_bytes, Preset};
use lopdf::{dictionary, Document, Object, Stream};
use std::io::{Read, Write};

// ---------- 1. images with /DecodeParms in array or reference form ----------

const W: usize = 600;
const H: usize = 400;

/// Photo-like RGB pixels: smooth gradients with noise, so they would be re-encoded as JPEG.
fn photo() -> Vec<u8> {
    let mut px = Vec::with_capacity(W * H * 3);
    let mut seed = 7u32;
    for y in 0..H {
        for x in 0..W {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = (seed >> 27) as u8;
            px.extend_from_slice(&[(x * 255 / W) as u8 ^ n, (y * 255 / H) as u8 ^ n, ((x + y) / 4) as u8 ^ n]);
        }
    }
    px
}

/// PNG "Sub" prediction per row, as `/Predictor 15` writers produce, then Flate.
fn png_predicted(px: &[u8]) -> Vec<u8> {
    let row = W * 3;
    let mut out = Vec::with_capacity(px.len() + H);
    for r in px.chunks(row) {
        out.push(1);
        for i in 0..row {
            out.push(r[i].wrapping_sub(if i >= 3 { r[i - 3] } else { 0 }));
        }
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&out).unwrap();
    z.finish().unwrap()
}

#[derive(Clone, Copy, Debug)]
enum Parms {
    Direct,
    Array,
    Reference,
}

/// One page drawing the image at one pixel per point, so no preset downsamples it.
fn image_pdf(parms: Parms) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let p = dictionary! { "Predictor" => 15, "Colors" => 3, "BitsPerComponent" => 8, "Columns" => W as i64 };
    let p = match parms {
        Parms::Direct => Object::Dictionary(p),
        Parms::Array => Object::Array(vec![Object::Dictionary(p)]),
        Parms::Reference => doc.add_object(p).into(),
    };
    let mut image = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => W as i64, "Height" => H as i64,
                      "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "FlateDecode", "DecodeParms" => p },
        png_predicted(&photo()),
    );
    image.allows_compression = false;
    let image_id = doc.add_object(image);
    let content = doc.add_object(Stream::new(dictionary! {}, format!("q {W} 0 0 {H} 0 0 cm /Im Do Q").into_bytes()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "Resources" => dictionary! { "XObject" => dictionary! { "Im" => image_id } },
        "MediaBox" => vec![0.into(), 0.into(), (W as i64).into(), (H as i64).into()],
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// The image's RGB pixels as a viewer would see them, decoding whatever the output holds.
fn shown_pixels(pdf: &[u8]) -> Vec<u8> {
    let doc = Document::load_mem(pdf).unwrap();
    let s = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .find(|s| s.dict.get(b"Subtype").and_then(|n| n.as_name()).ok() == Some(b"Image"))
        .expect("an image");
    assert_eq!((s.dict.get(b"Width").unwrap().as_i64().unwrap(), s.dict.get(b"Height").unwrap().as_i64().unwrap()), (W as i64, H as i64));
    match s.dict.get(b"Filter").unwrap().as_name().unwrap() {
        b"DCTDecode" => image::load_from_memory(&s.content).unwrap().to_rgb8().into_raw(),
        b"FlateDecode" => {
            let mut data = Vec::new();
            flate2::read::ZlibDecoder::new(&s.content[..]).read_to_end(&mut data).unwrap();
            if !s.dict.has(b"DecodeParms") {
                return data;
            }
            // Undo the Sub predictor (the only one this test writes).
            let row = W * 3;
            let mut px = Vec::with_capacity(W * H * 3);
            for r in data.chunks(row + 1) {
                assert_eq!(r[0], 1);
                let start = px.len();
                for i in 0..row {
                    let left = if i >= 3 { px[start + i - 3] } else { 0 };
                    px.push(r[1 + i].wrapping_add(left));
                }
            }
            px
        }
        f => panic!("unexpected filter {}", String::from_utf8_lossy(f)),
    }
}

fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

#[test]
fn predicted_images_keep_their_picture_whatever_form_the_parameters_take() {
    let original = photo();
    for parms in [Parms::Direct, Parms::Array, Parms::Reference] {
        let input = image_pdf(parms);
        assert!(mean_difference(&shown_pixels(&input), &original) < 0.01, "{parms:?}: test file itself is wrong");
        for preset in [Preset::Lossless, Preset::Balanced, Preset::Small] {
            let Ok((out, _)) = compress_bytes(&input, &preset.options()) else { continue };
            let d = mean_difference(&shown_pixels(&out), &original);
            // JPEG at quality 60 stays well under 10 on this picture; a scrambled image is near 80.
            assert!(d < 10.0, "{parms:?} with {preset:?}: picture changed (mean difference {d:.1})");
        }
    }
}

// ---------- 2. the font in use is restored at Q ----------

const GLYPHS: u16 = 12;

/// A TrueType font whose glyph g (1..12) is a square of side 10·g. `variant` makes two
/// fonts with different bytes, so they are not merged as duplicates.
fn font(variant: u8) -> Vec<u8> {
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for g in 0..GLYPHS {
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        if g == 0 {
            continue;
        }
        let s = (g as i16) * 10;
        glyf.extend_from_slice(&1i16.to_be_bytes());
        for v in [0, 0, s, s] {
            glyf.extend_from_slice(&v.to_be_bytes());
        }
        glyf.extend_from_slice(&3u16.to_be_bytes());
        glyf.extend_from_slice(&0u16.to_be_bytes());
        glyf.extend_from_slice(&[0x01; 4]);
        for v in [0, s, s, 0, 0, 0, s, s] {
            glyf.extend_from_slice(&v.to_be_bytes());
        }
        glyf.resize((glyf.len() + 3) & !3, 0);
    }
    loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
    let mut head = vec![0u8; 54];
    head[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    head[4] = variant;
    head[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    head[18..20].copy_from_slice(&1000u16.to_be_bytes());
    head[50..52].copy_from_slice(&1i16.to_be_bytes());
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
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&((12 + 16 * tables.len() + body.len()) as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        body.resize((body.len() + 3) & !3, 0);
    }
    out.extend_from_slice(&body);
    out
}

fn add_font(doc: &mut Document, name: &str, variant: u8) -> lopdf::ObjectId {
    let program = font(variant);
    let mut file = Stream::new(dictionary! { "Length1" => program.len() as i64 }, program);
    file.compress().unwrap();
    let file_id = doc.add_object(file);
    let descriptor = doc.add_object(dictionary! {
        "Type" => "FontDescriptor", "FontName" => name, "Flags" => 32,
        "FontBBox" => vec![0.into(), 0.into(), 1000.into(), 1000.into()], "ItalicAngle" => 0,
        "Ascent" => 1000, "Descent" => 0, "CapHeight" => 1000, "StemV" => 80, "FontFile2" => file_id,
    });
    let cid = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => name,
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "FontDescriptor" => descriptor, "CIDToGIDMap" => "Identity",
    });
    doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => name, "Encoding" => "Identity-H",
        "DescendantFonts" => vec![cid.into()],
    })
}

/// F1 draws glyph 3; F2 is selected inside q … Q; after Q the font is F1 again, which draws glyph 5.
fn q_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let f1 = add_font(&mut doc, "Alpha", 1);
    let f2 = add_font(&mut doc, "Beta", 2);
    let content = b"BT /F1 12 Tf 72 700 Td <0003> Tj ET q BT /F2 12 Tf 72 680 Td <0004> Tj ET Q BT 72 660 Td <0005> Tj ET";
    let content = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content,
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => f1, "F2" => f2 } },
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// Glyphs with outlines in each embedded font, by the font's name (subset tag removed).
fn glyphs_by_font(pdf: &[u8]) -> Vec<(String, Vec<u16>)> {
    let doc = Document::load_mem(pdf).unwrap();
    let mut out = Vec::new();
    for obj in doc.objects.values() {
        let Ok(d) = obj.as_dict() else { continue };
        if d.get(b"Type").and_then(|t| t.as_name()).ok() != Some(b"FontDescriptor") {
            continue;
        }
        let name = String::from_utf8_lossy(d.get(b"FontName").unwrap().as_name().unwrap()).into_owned();
        let name = name.rsplit('+').next().unwrap().to_string();
        let file = doc.get_object(d.get(b"FontFile2").unwrap().as_reference().unwrap()).unwrap().as_stream().unwrap();
        let f = file.decompressed_content().unwrap_or_else(|_| file.content.clone());
        let n = u16::from_be_bytes([f[4], f[5]]) as usize;
        let loca = (0..n)
            .find_map(|i| {
                let r = &f[12 + 16 * i..28 + 16 * i];
                (&r[0..4] == b"loca").then(|| {
                    let off = u32::from_be_bytes(r[8..12].try_into().unwrap()) as usize;
                    &f[off..off + u32::from_be_bytes(r[12..16].try_into().unwrap()) as usize]
                })
            })
            .unwrap();
        let at = |g: usize| u32::from_be_bytes(loca[g * 4..g * 4 + 4].try_into().unwrap());
        out.push((name, (0..GLYPHS).filter(|g| at(*g as usize + 1) > at(*g as usize)).collect()));
    }
    out.sort();
    out
}

#[test]
fn font_selected_inside_q_does_not_steal_glyphs_after_q() {
    let input = q_pdf();
    let (out, report) = compress_bytes(&input, &Preset::Lossless.options()).unwrap();
    assert_eq!(report.fonts_subset, 2);
    let glyphs = glyphs_by_font(&out);
    assert_eq!(glyphs, vec![("Alpha".to_string(), vec![3, 5]), ("Beta".to_string(), vec![4])]);
}
