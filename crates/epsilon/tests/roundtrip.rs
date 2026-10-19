use epsilon::{analyze_bytes, compress_bytes, Preset, Status};
use lopdf::{dictionary, Document, Object, Stream};

/// One letter-size page drawing a 2000 px photo-like image in a 2 x 2 inch box and a
/// 1200 px two-colour graphic in a 4 x 4 inch box, plus a line of text.
fn sample_pdf() -> Vec<u8> {
    sample_pdf_with(
        b"q 144 0 0 144 72 600 cm /Photo Do Q q 288 0 0 288 72 200 cm /Graphic Do Q \
          BT /F1 12 Tf 72 100 Td (Keep what matters) Tj ET",
        false,
    )
}

/// The same page with other content, and optionally an embedded thumbnail.
fn sample_pdf_with(content: &[u8], thumbnail: bool) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    let (pw, ph) = (2000u32, 2000u32);
    let mut photo = Vec::with_capacity((pw * ph * 3) as usize);
    let mut seed = 12345u32;
    for y in 0..ph {
        for x in 0..pw {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let noise = (seed >> 24) as u8 / 8;
            photo.push(((x / 8) as u8).wrapping_add(noise));
            photo.push(((y / 8) as u8).wrapping_add(noise));
            photo.push((((x + y) / 16) as u8).wrapping_add(noise));
        }
    }
    let mut photo = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => pw as i64, "Height" => ph as i64,
                      "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
        photo,
    );
    photo.compress().unwrap();
    let photo_id = doc.add_object(photo);

    let gw = 1200u32;
    let graphic: Vec<u8> = (0..gw * gw).map(|i| if (i / gw / 100 + i % gw / 100) % 2 == 0 { 0 } else { 255 }).collect();
    let mut graphic = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => gw as i64, "Height" => gw as i64,
                      "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 },
        graphic,
    );
    graphic.compress().unwrap();
    let graphic_id = doc.add_object(graphic);

    let font_id = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
    let resources_id = doc.add_object(dictionary! {
        "XObject" => dictionary! { "Photo" => photo_id, "Graphic" => graphic_id },
        "Font" => dictionary! { "F1" => font_id },
    });
    let mut page = dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id, "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
    };
    if thumbnail {
        let thumb = Stream::new(
            dictionary! { "Width" => 60, "Height" => 80, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
            vec![128u8; 60 * 80 * 3],
        );
        page.set("Thumb", doc.add_object(thumb));
    }
    let page_id = doc.add_object(page);
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }));
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn images(bytes: &[u8]) -> Vec<(i64, i64, Vec<u8>)> {
    let doc = Document::load_mem(bytes).unwrap();
    let mut v: Vec<_> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| s.dict.get(b"Subtype").and_then(|n| n.as_name()).ok() == Some(b"Image"))
        .map(|s| {
            (
                s.dict.get(b"Width").unwrap().as_i64().unwrap(),
                s.dict.get(b"Height").unwrap().as_i64().unwrap(),
                s.dict.get(b"Filter").unwrap().as_name().unwrap().to_vec(),
            )
        })
        .collect();
    v.sort();
    v
}

#[test]
fn balanced_downsamples_photo_to_drawn_size_and_keeps_graphic_lossless() {
    let input = sample_pdf();
    let (out, report) = compress_bytes(&input, &Preset::Balanced.options()).unwrap();
    assert!(!report.kept_original);
    assert!(out.len() < input.len() / 4, "{} -> {}", input.len(), out.len());
    assert_eq!(report.pages, 1);
    // 2 inches at 150 dpi is 300 px; 4 inches at 150 dpi is 600 px.
    assert_eq!(
        images(&out),
        vec![(300, 300, b"DCTDecode".to_vec()), (600, 600, b"FlateDecode".to_vec())]
    );
    let text = Document::load_mem(&out).unwrap().extract_text(&[1]).unwrap();
    assert!(text.contains("Keep what matters"));
}

#[test]
fn lossless_never_touches_pixels() {
    let input = sample_pdf();
    let (out, _) = compress_bytes(&input, &Preset::Lossless.options()).unwrap();
    assert_eq!(
        images(&out),
        vec![(1200, 1200, b"FlateDecode".to_vec()), (2000, 2000, b"FlateDecode".to_vec())]
    );
}

#[test]
fn print_keeps_images_already_at_or_below_300_dpi() {
    let input = sample_pdf();
    let (out, _) = compress_bytes(&input, &Preset::Print.options()).unwrap();
    // The graphic is drawn at exactly 300 dpi, so it keeps its size; the photo
    // (1000 dpi as drawn) drops to 600 px.
    let imgs = images(&out);
    assert!(imgs.contains(&(600, 600, b"DCTDecode".to_vec())), "{imgs:?}");
    assert!(imgs.iter().any(|(w, _, _)| *w == 1200), "{imgs:?}");
}

#[test]
fn rejects_garbage() {
    assert!(compress_bytes(b"not a pdf", &Preset::Balanced.options()).is_err());
}

#[test]
fn analysis_accounts_for_every_byte_and_matches_real_runs() {
    let input = sample_pdf();
    let a = analyze_bytes(&input).unwrap();
    assert_eq!(a.file.pages, 1);
    let total: u64 = a.categories.iter().map(|c| c.bytes).sum();
    assert_eq!(total, input.len() as u64, "{:?}", a.categories);

    // Each preset's measured size is exactly what compressing produces.
    for (preset, result) in Preset::ALL.iter().zip(&a.presets) {
        let (out, _) = compress_bytes(&input, &preset.options()).unwrap();
        assert_eq!(result.output_bytes, out.len() as u64, "{preset:?}");
        let parts: i64 = result.savings.iter().map(|s| s.bytes).sum();
        assert_eq!(parts, input.len() as i64 - out.len() as i64, "{preset:?}");
    }

    // The photo is 2000 px drawn at 2 inches: 1000 ppi. The graphic is 1200 px at 4 inches.
    let ppi: Vec<u32> = a.images.iter().map(|i| i.ppi.unwrap().round() as u32).collect();
    assert!(ppi.contains(&1000) && ppi.contains(&300), "{ppi:?}");
    assert!(a.images.iter().all(|i| i.visible == Some(1.0) && i.pages == vec![1]));
    let oversized = a.findings.iter().find(|f| f.kind == "oversizedImages").expect("oversized finding");
    assert_eq!(oversized.status, Status::Fixed);
    assert_eq!(oversized.pages, vec![1]);
    assert_eq!(oversized.savings[0], 0, "lossless never touches images");
    assert!(oversized.savings[2] > 0);

    let page = &a.pages[0];
    assert_eq!(page.image_objs.len(), 2);
    assert_eq!(page.font_objs.len(), 1);
    assert!(page.issues.iter().any(|i| i.kind == "oversizedImage"));
    assert!(page.savings[2] > 0);
}

#[test]
fn analysis_finds_thumbnails_and_images_off_the_page() {
    // The photo is drawn 6 inches wide starting 1 inch from the right edge: a sixth is visible.
    let input = sample_pdf_with(b"q 432 0 0 432 540 100 cm /Photo Do Q", true);
    let a = analyze_bytes(&input).unwrap();
    let photo = a.images.iter().find(|i| i.width == 2000).unwrap();
    let visible = photo.visible.unwrap();
    assert!((visible - 72.0 / 432.0).abs() < 0.01, "{visible}");
    assert!(a.findings.iter().any(|f| f.kind == "offpageImages" && f.status == Status::NotYet));
    let thumbs = a.findings.iter().find(|f| f.kind == "thumbnails").expect("thumbnail finding");
    assert!(thumbs.savings.iter().all(|s| *s > 0), "{:?}", thumbs.savings);
    // The graphic is listed in resources but never drawn.
    let graphic = a.images.iter().find(|i| i.width == 1200).unwrap();
    assert!(graphic.pages.is_empty() && graphic.ppi.is_none());
}

#[test]
fn analysis_refuses_garbage() {
    assert!(analyze_bytes(b"not a pdf").is_err());
}
