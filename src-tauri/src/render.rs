//! Page thumbnails, rendered in a separate process.
//!
//! Rendering means interpreting everything a page draws, which is far more code reading
//! untrusted input than the engine's bounded analysis. The renderer (hayro, pure Rust) has
//! no limits of its own: a hostile file can make it allocate gigabytes or run for minutes.
//! So it never runs inside the app. The app starts its own executable again with
//! `--render-thumbnails`; that process caps its own memory (see `CappedAlloc` in main.rs)
//! and is killed if it stalls. A hostile file costs some thumbnails, never the app.
//!
//! Protocol: the child gets the file and width as arguments and page numbers on stdin, one
//! per line. For each page it writes a frame to stdout: page number and length as big-endian
//! u32, then the PNG (length 0 when that page could not be drawn). With `-` as the file, the
//! PDF itself comes first on stdin, as a big-endian u64 length and the bytes, so a compressed
//! copy that exists only in memory is never written to disk.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};

/// The argument that starts the renderer instead of the app.
pub const FLAG: &str = "--render-thumbnails";
/// Memory the renderer process may use.
pub const MEMORY: usize = 1 << 30;
/// The renderer is killed when a page takes longer than this (including reading the file)...
const PAGE_TIMEOUT: Duration = Duration::from_secs(12);
/// ...or the whole batch takes longer than this.
const BATCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Thumbnails are at most this many times taller than wide, whatever the page claims.
const MAX_ASPECT: f32 = 4.0;

/// Where the renderer reads the PDF from.
pub enum Source {
    File(PathBuf),
    /// A document held only in memory, such as a compressed copy not yet saved.
    Bytes(Arc<Vec<u8>>),
}

/// Child side: renders the requested pages of `path` (or of the PDF on stdin, for `-`),
/// `width` pixels wide.
pub fn child(path: &Path, width: f32) -> i32 {
    let mut input = std::io::stdin().lock();
    let bytes = if path == Path::new("-") {
        let mut len = [0u8; 8];
        if input.read_exact(&mut len).is_err() {
            return 1;
        }
        let mut bytes = Vec::new();
        if (&mut input).take(u64::from_be_bytes(len)).read_to_end(&mut bytes).is_err() {
            return 1;
        }
        bytes
    } else {
        match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return 1,
        }
    };
    let Ok(pdf) = Pdf::new(bytes) else { return 1 };
    let pages = pdf.pages();
    let cache = RenderCache::new();
    let settings = InterpreterSettings::default();
    let mut out = std::io::stdout().lock();
    for line in input.lines() {
        let Ok(line) = line else { break };
        let Ok(number) = line.trim().parse::<u32>() else { continue };
        let png = (number as usize)
            .checked_sub(1)
            .and_then(|i| pages.get(i))
            .and_then(|page| {
                let (w, h) = page.render_dimensions();
                if w <= 0.0 || h <= 0.0 {
                    return None;
                }
                let scale = (width / w).min(width * MAX_ASPECT / h);
                let pixmap = render(page, &cache, &settings, &RenderSettings::default(), &PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE });
                pixmap.into_png().ok()
            })
            .unwrap_or_default();
        let header = [number.to_be_bytes(), (png.len() as u32).to_be_bytes()].concat();
        if out.write_all(&header).and_then(|_| out.write_all(&png)).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
    0
}

/// Parent side: renders `pages` of `source` in a child process and calls `each` with every
/// image as it arrives (`None` for a page that could not be drawn). Pages not done when the
/// child dies or is killed are reported as `None`.
pub fn pages(source: Source, pages: &[u32], width: u32, mut each: impl FnMut(u32, Option<Vec<u8>>)) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let (file_arg, bytes) = match source {
        Source::File(p) => (p.into_os_string(), None),
        Source::Bytes(b) => ("-".into(), Some(b)),
    };
    let mut child = Command::new(exe)
        .arg(FLAG)
        .arg(file_arg)
        .arg(width.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped");
    let mut stdout = child.stdout.take().expect("piped");
    let list: String = pages.iter().map(|p| format!("{p}\n")).collect();
    // Written from a thread so a child that stops reading cannot block the app.
    std::thread::spawn(move || {
        if let Some(b) = bytes {
            if stdin.write_all(&(b.len() as u64).to_be_bytes()).and_then(|_| stdin.write_all(&b)).is_err() {
                return;
            }
        }
        let _ = stdin.write_all(list.as_bytes());
    });

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        let mut header = [0u8; 8];
        if stdout.read_exact(&mut header).is_err() {
            break;
        }
        let page = u32::from_be_bytes(header[0..4].try_into().unwrap());
        let len = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
        if len > 64 << 20 {
            break;
        }
        let mut png = vec![0u8; len];
        if stdout.read_exact(&mut png).is_err() {
            break;
        }
        if tx.send((page, png)).is_err() {
            break;
        }
    });

    let started = Instant::now();
    let mut done = std::collections::HashSet::new();
    while done.len() < pages.len() {
        let left = BATCH_TIMEOUT.saturating_sub(started.elapsed());
        match rx.recv_timeout(PAGE_TIMEOUT.min(left)) {
            Ok((page, png)) => {
                done.insert(page);
                each(page, (!png.is_empty()).then_some(png));
            }
            // Stalled, out of time, or gone (crashed, or out of memory).
            Err(_) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    for p in pages {
        if !done.contains(p) {
            each(*p, None);
        }
    }
    Ok(())
}
