//! The renderer process on its own: it draws a page, and a page that needs too much memory
//! ends it quietly (an exit code, not a crash) instead of taking the computer's memory.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn pdf(dir: &Path, name: &str, images: usize) -> PathBuf {
    let mut objs: Vec<Vec<u8>> = Vec::new();
    let names: String = (0..images).map(|i| format!("/I{i} {} 0 R ", 5 + i)).collect();
    objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objs.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    objs.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << {names}>> >> /Contents 4 0 R >>").into_bytes());
    let draw: String = (0..images).map(|i| format!("q 612 0 0 792 0 0 cm /I{i} Do Q ")).collect::<String>() + "0 0 1 rg 100 100 200 200 re f";
    objs.push([format!("<< /Length {} >>\nstream\n", draw.len()).into_bytes(), draw.into_bytes(), b"\nendstream".to_vec()].concat());
    // 12,000 × 12,000 RGB images of zeros: a few megabytes on disk, 432 MB each once decoded.
    let side = 12_000usize;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let row = vec![0u8; side * 3];
    for _ in 0..side {
        z.write_all(&row).unwrap();
    }
    let data = z.finish().unwrap();
    for _ in 0..images {
        let head = format!("<< /Type /XObject /Subtype /Image /Width {side} /Height {side} /BitsPerComponent 8 /ColorSpace /DeviceRGB /Filter /FlateDecode /Length {} >>\nstream\n", data.len());
        objs.push([head.into_bytes(), data.clone(), b"\nendstream".to_vec()].concat());
    }
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
    let p = dir.join(name);
    std::fs::write(&p, out).unwrap();
    p
}

/// Runs the renderer on page 1; returns its exit code and the PNG lengths it sent.
fn render(file: &Path) -> (Option<i32>, Vec<usize>, Duration) {
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pdf-epsilon"))
        .args(["--render-thumbnails".as_ref(), file.as_os_str(), "160".as_ref()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"1\n").unwrap();
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let status = child.wait().unwrap();
    let mut frames = Vec::new();
    let mut at = 0;
    while at + 8 <= out.len() {
        let len = u32::from_be_bytes(out[at + 4..at + 8].try_into().unwrap()) as usize;
        frames.push(len);
        at += 8 + len;
    }
    (status.code(), frames, started.elapsed())
}

#[test]
fn draws_a_page_and_ends_quietly_when_a_page_needs_too_much_memory() {
    let dir = std::env::temp_dir().join(format!("pdf-epsilon-renderer-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let (code, frames, _) = render(&pdf(&dir, "plain.pdf", 0));
    assert_eq!(code, Some(0));
    assert!(frames.len() == 1 && frames[0] > 0, "{frames:?}");

    // Six such images reached 2.6 GB in 0.7.0. Now the renderer ends itself (code 3 from its
    // watchdog, 4 from its allocator), well before using that much.
    let (code, frames, took) = render(&pdf(&dir, "heavy.pdf", 6));
    assert!(matches!(code, Some(3) | Some(4)), "ended with {code:?}, sent {frames:?}");
    assert!(took < Duration::from_secs(30), "{took:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}
