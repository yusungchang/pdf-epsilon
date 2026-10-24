//! Hostile PDFs: each targets one way a file could exhaust memory, loop or overflow the stack
//! while PDF Epsilon reads it: analysis and compression. Every case must
//! finish quickly with a result or a clean error. (Thumbnails are rendered in a separate,
//! memory-capped process; see src-tauri/src/render.rs.)
//!
//! The work runs on a thread with a 1 MiB stack, smaller than any platform default (Windows
//! gives its main thread 1 MiB; macOS and Linux give more), so deep recursion that passes here
//! passes everywhere. Bombs are sized just past each limit in limits.rs: decoding is refused
//! at the limit, so a bigger bomb behaves the same.

use std::io::Write;
use std::time::{Duration, Instant};

use epsilon::{analyze_bytes, compress_bytes, Preset};

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// zlib stream of `n` zero bytes, without holding them all in memory.
fn zeros(n: usize) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    let chunk = vec![0u8; 1 << 20];
    for _ in 0..n / chunk.len() {
        e.write_all(&chunk).unwrap();
    }
    e.finish().unwrap()
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nendstream");
    s
}

/// A one-page PDF. `objects` are numbered from 5; 1–4 are the catalog, page tree, page and content.
fn pdf(resources: &str, content: Vec<u8>, catalog: &str, objects: Vec<Vec<u8>>) -> Vec<u8> {
    pdf_with_page("", resources, content, catalog, objects)
}

/// The same, with more entries in the page dictionary (`/Annots`, say).
fn pdf_with_page(page: &str, resources: &str, content: Vec<u8>, catalog: &str, objects: Vec<Vec<u8>>) -> Vec<u8> {
    let mut objs: Vec<Vec<u8>> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {page} /Resources << {resources} >> /Contents 4 0 R >>").into_bytes(),
        content,
    ];
    objs.extend(objects);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn page_with(content: &[u8]) -> Vec<u8> {
    pdf("", stream("/Filter /FlateDecode", &zlib(content)), "", vec![])
}

const TYPE0: &str = "/Font << /F1 5 0 R >>";
fn type0(program: Vec<u8>, map: Vec<u8>) -> Vec<Vec<u8>> {
    vec![
        b"<< /Type /Font /Subtype /Type0 /BaseFont /X /Encoding /Identity-H /DescendantFonts [6 0 R] >>".to_vec(),
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /X /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 7 0 R /CIDToGIDMap 9 0 R >>".to_vec(),
        b"<< /Type /FontDescriptor /FontName /X /Flags 32 /FontBBox [0 0 1 1] /ItalicAngle 0 /Ascent 1 /Descent 0 /CapHeight 1 /StemV 1 /FontFile2 8 0 R >>".to_vec(),
        program,
        map,
    ]
}

fn cases() -> Vec<(&'static str, Vec<u8>)> {
    let mb = 1 << 20;
    let draw_image = b"q 600 0 0 700 0 0 cm /Im Do Q".to_vec();
    let mut chain = Vec::new();
    for i in 0..2000 {
        chain.push(stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Fm {} 0 R >> >>", 6 + i), b"/Fm Do"));
    }
    let mut bad_font = b"\0\x01\0\0\0\x04\0\x40\0\x02\0\0".to_vec();
    for (tag, off, len) in [(b"head", 76u32, 54u32), (b"maxp", 130, 6), (b"loca", 136, 12), (b"glyf", 0xFFFF_FFF0, 0xFFFF)] {
        bad_font.extend_from_slice(tag);
        bad_font.extend_from_slice(&[0; 4]);
        bad_font.extend_from_slice(&off.to_be_bytes());
        bad_font.extend_from_slice(&len.to_be_bytes());
    }
    bad_font.extend_from_slice(&[0; 54]);
    bad_font.extend_from_slice(b"\0\0\x50\0\xff\xff");
    bad_font.extend_from_slice(&[0; 12]);
    // Ten forms, each drawing the next ten times: ten billion draws from a 2 KB file.
    let mut fan = Vec::new();
    for i in 0..10 {
        let next = if i < 9 { format!("/Resources << /XObject << /Fm {} 0 R >> >>", 6 + i) } else { String::new() };
        let body: &[u8] = if i < 9 { b"/Fm Do /Fm Do /Fm Do /Fm Do /Fm Do /Fm Do /Fm Do /Fm Do /Fm Do /Fm Do" } else { b"0 0 1 1 re f" };
        fan.push(stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 1 1] {next}"), body));
    }
    let xref_predictor = {
        let mut b = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n".to_vec();
        let x = b.len();
        b.extend_from_slice(b"3 0 obj\n");
        b.extend(stream("/Type /XRef /Size 4 /W [1 4 1] /Root 1 0 R /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 999999999999 >>", &zlib(&[2; 64])));
        b.extend_from_slice(format!("\nendobj\nstartxref\n{x}\n%%EOF\n").as_bytes());
        b
    };

    vec![
        ("content over its limit", pdf("", stream("/Filter /FlateDecode", &zeros(65 * mb)), "", vec![])),
        ("object stream over its limit", pdf("", stream("", b"BT ET"), "", vec![stream("/Type /ObjStm /N 1 /First 4 /Filter /FlateDecode", &zeros(257 * mb))])),
        ("image larger than it claims", pdf("/XObject << /Im 5 0 R >>", stream("", &draw_image), "", vec![stream("/Type /XObject /Subtype /Image /Width 100 /Height 100 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode", &zeros(64 * mb))])),
        ("form over its limit", pdf("/XObject << /Fm 5 0 R >>", stream("", b"/Fm Do"), "", vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Filter /FlateDecode", &zeros(65 * mb))])),
        ("glyph map over its limit", pdf(TYPE0, stream("", b"BT /F1 12 Tf <0001> Tj ET"), "", type0(stream("/Length1 4", b"\0\x01\0\0"), stream("/Filter /FlateDecode", &zeros(17 * mb))))),
        ("metadata over its limit", pdf("", stream("", b"BT ET"), "/Metadata 5 0 R", vec![stream("/Type /Metadata /Subtype /XML /Filter /FlateDecode", &zeros(17 * mb))])),
        ("a million drawing operations", page_with(&b"q\n".repeat(1_100_000))),
        ("predictor with absurd columns", pdf("/XObject << /Im 5 0 R >>", stream("", &draw_image), "", vec![stream("/Type /XObject /Subtype /Image /Width 100 /Height 100 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns 2147483647 /Colors 4 >>", &zlib(&[2; 5000]))])),
        ("xref stream with absurd predictor", xref_predictor),
        ("JPEG claiming 65000 x 65000", pdf("/XObject << /Im 5 0 R >>", stream("", &draw_image), "", vec![stream("/Type /XObject /Subtype /Image /Width 65000 /Height 65000 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode", b"\xff\xd8\xff\xc0\x00\x11\x08\xfd\xe8\xfd\xe8\x03\x01\x22\x00\x02\x11\x01\x03\x11\x01\xff\xd9")])),
        ("image with too little data", pdf("/XObject << /Im 5 0 R >>", stream("", &draw_image), "", vec![stream("/Type /XObject /Subtype /Image /Width 2000000000 /Height 2000000000 /ColorSpace /DeviceRGB /BitsPerComponent 8", &[0; 100])])),
        ("form drawing itself", pdf("/XObject << /Fm 5 0 R >>", stream("", b"/Fm Do"), "", vec![stream("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /Fm 5 0 R >> >>", b"/Fm Do /Fm Do")])),
        ("2000 nested forms", pdf("/XObject << /Fm 5 0 R >>", stream("", b"/Fm Do"), "", chain)),
        ("forms fanned out ten levels deep", pdf("/XObject << /Fm 5 0 R >>", stream("", b"/Fm Do"), "", fan)),
        // The structure check and the image-size walk read these; each loops back to itself.
        ("bookmark that is its own next", pdf("", stream("", b"BT ET"), "/Outlines 5 0 R", vec![b"<< /First 6 0 R >>".to_vec(), b"<< /Title (a) /Next 6 0 R /First 6 0 R >>".to_vec()])),
        ("form field that is its own kid", pdf("", stream("", b"BT ET"), "/AcroForm << /Fields [5 0 R] >>", vec![b"<< /T (a) /Kids [5 0 R 5 0 R] >>".to_vec()])),
        ("name tree containing itself", pdf("", stream("", b"BT ET"), "/Names << /EmbeddedFiles 5 0 R /Dests 5 0 R >>", vec![b"<< /Kids [5 0 R 5 0 R] /Names [(a) 5 0 R] >>".to_vec()])),
        ("appearance drawing itself", pdf_with_page("/Annots [10 0 R]", TYPE0, stream("", b"BT /F1 12 Tf <0001> Tj ET"), "", {
            // 5–9: a Type 0 font; 10: a stamp; 11: its appearance, which draws itself twice.
            let mut o = type0(stream("/Length1 4", b"\0\x01\0\0"), b"/Identity".to_vec());
            o.push(b"<< /Type /Annot /Subtype /Stamp /Rect [0 0 9 9] /AP << /N 11 0 R /D << /a 11 0 R /b 11 0 R >> >> >>".to_vec());
            o.push(stream("/Type /XObject /Subtype /Form /BBox [0 0 9 9] /Resources << /XObject << /Fm 11 0 R >> /Pattern << /P 11 0 R >> /Font << /F1 5 0 R >> >>", b"/Fm Do /Fm Do BT /F1 9 Tf <0001> Tj ET"));
            o
        })),
        ("page tree containing itself", {
            let mut p = page_with(b"BT ET");
            let at = p.windows(9).position(|w| w == b"/Kids [3 ").unwrap();
            p.splice(at..at + 9, b"/Kids [2 0 R 3 ".iter().copied());
            p
        }),
        ("deeply nested objects", pdf("", stream("", b"BT ET"), "", vec![[b"[".repeat(100_000), b"]".repeat(100_000)].concat(), [b"<< /A ".repeat(20_000), b"1".to_vec(), b" >>".repeat(20_000)].concat()])),
        ("deeply nested content", page_with(&[b"[".repeat(100_000), b"]".repeat(100_000), b" TJ".to_vec()].concat())),
        ("font with a lying table directory", pdf(TYPE0, stream("", b"BT /F1 12 Tf <00010002FFFF> Tj ET"), "", type0(stream(&format!("/Length1 {}", bad_font.len()), &bad_font), b"/Identity".to_vec()))),
        ("object stream claiming two million objects", pdf("", stream("", b"BT ET"), "", vec![stream("/Type /ObjStm /N 2000000 /First 10", b"1 0 null")])),
        ("truncated file", page_with(b"BT ET")[..200].to_vec()),
        ("not a PDF", b"%PDF-1.7\n\xff\xfe garbage".to_vec()),
    ]
}

#[test]
fn hostile_files_finish_quickly_on_a_small_stack() {
    let worker = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let mut slow = Vec::new();
            for (name, bytes) in cases() {
                let started = Instant::now();
                // Either outcome is fine; panicking, aborting or hanging is not.
                let _ = analyze_bytes(&bytes);
                let _ = compress_bytes(&bytes, &Preset::Small.options());
                let took = started.elapsed();
                if took > Duration::from_secs(20) {
                    slow.push(format!("{name}: {took:?}"));
                }
            }
            slow
        })
        .unwrap();
    let slow = worker.join().expect("a hostile file crashed the worker");
    assert!(slow.is_empty(), "too slow: {slow:?}");
}

/// lopdf reads leniently: an object it cannot parse, or an object stream over the size limit, is
/// left out of the document instead of failing the load. Saving that document would quietly drop
/// a font or an annotation while every page is still there. Such files must not be written.
#[test]
fn objects_lopdf_cannot_read_are_never_silently_dropped() {
    let font_used = b"BT /F1 24 Tf 72 700 Td (Hello) Tj ET";
    let cases = [
        ("font object with a syntax error", pdf("/Font << /F1 5 0 R >>", stream("", font_used), "", vec![b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica ]] >>".to_vec()])),
        ("content stream without a length", {
            let mut b = pdf("/Font << /F1 5 0 R >>", stream("", font_used), "", vec![b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()]);
            let at = b.windows(8).position(|w| w == b"/Length ").unwrap();
            b[at + 1] = b'Q';
            b
        }),
        ("content deleted by a later revision", {
            let mut b = pdf("/Font << /F1 5 0 R >>", stream("", font_used), "", vec![b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec()]);
            let prev = String::from_utf8_lossy(&b).rfind("startxref\n").map(|i| {
                let t = String::from_utf8_lossy(&b[i + 10..]).to_string();
                t.lines().next().unwrap().to_string()
            }).unwrap();
            let at = b.len();
            b.extend_from_slice(format!("xref\n4 1\n0000000000 00001 f \ntrailer\n<< /Size 6 /Root 1 0 R /Prev {prev} >>\nstartxref\n{at}\n%%EOF\n").as_bytes());
            b
        }),
        ("font inside an object stream over its limit", pdf("/Font << /F1 5 0 R >>", stream("", font_used), "", vec![stream("/Type /ObjStm /N 1 /First 4 /Filter /FlateDecode", &zeros(257 << 20))])),
    ];
    for (name, bytes) in cases {
        if let Ok((out, report)) = compress_bytes(&bytes, &Preset::Balanced.options()) {
            // Keeping the original untouched is fine; writing a copy without the object is not.
            assert!(report.kept_original || out == bytes, "{name}: wrote a copy that lost an object");
        }
    }
}
