//! Page thumbnails, rendered in a separate process.
//!
//! Rendering means interpreting everything a page draws, which is far more code reading
//! untrusted input than the engine's bounded analysis. The renderer (hayro, pure Rust) has
//! no limits of its own: a hostile file can make it allocate gigabytes or run for minutes.
//! So it never runs inside the app. The app starts its own executable again with
//! `--render-thumbnails`. That process limits itself twice: a counting allocator refuses
//! allocations past 1 GB of live heap (`CappedAlloc` in main.rs), and a watchdog thread ends
//! the process once its resident memory passes 1.5 GB or its time runs out. The second is
//! needed because memory a renderer frees is not always returned to the system: in 0.7.0 a
//! 22 MB file with large images held 5.3 GB while the live heap stayed under 1 GB. Either way
//! the process exits quietly, without a crash report, and the app kills it if it stalls.
//! At most two run at once, and a newer request of the same kind (say, thumbnails of another
//! file) cancels the older one. A hostile file costs some pictures, never the app.
//!
//! Protocol: the child gets the file and width as arguments and page numbers on stdin, one
//! per line. For each page it writes a frame to stdout: page number and length as big-endian
//! u32, then the PNG (length 0 when that page could not be drawn). With `-` as the file, the
//! PDF itself comes first on stdin, as a big-endian u64 length and the bytes, so a compressed
//! copy that exists only in memory is never written to disk.

use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};

/// The argument that starts the renderer instead of the app.
pub const FLAG: &str = "--render-thumbnails";
/// Live heap the renderer process may hold (enforced by the allocator).
pub const MEMORY: usize = 1 << 30;
/// Resident memory the renderer process may reach (enforced by its watchdog).
const RESIDENT: u64 = 3 << 29;
/// Renderer processes running at once.
const AT_ONCE: usize = 2;
/// Files larger than this are not drawn: reading and parsing them alone passes the limits.
pub const LARGEST: u64 = 400 << 20;
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
    // The watchdog: memory, and time, so a renderer whose app has gone does not run on.
    std::thread::spawn(|| {
        let started = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(20));
            if peak_resident() > RESIDENT || started.elapsed() > BATCH_TIMEOUT + Duration::from_secs(5) {
                quit(3);
            }
        }
    });
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

/// Ends the process at once: no destructors, no exit handlers, no crash report. Safe to call
/// from the allocator and from any thread.
pub fn quit(code: i32) -> ! {
    #[cfg(unix)]
    unsafe {
        libc::_exit(code)
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code as u32);
        loop {}
    }
}

/// The most memory this process has held resident, in bytes.
fn peak_resident() -> u64 {
    #[cfg(unix)]
    {
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
            return 0;
        }
        let max = usage.ru_maxrss.max(0) as u64;
        // macOS reports bytes, Linux kilobytes.
        if cfg!(target_os = "macos") {
            max
        } else {
            max * 1024
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } == 0 {
            return 0;
        }
        c.PeakWorkingSetSize as u64
    }
}

/// The executable to start as the renderer: this one. Tests point it at the built app.
fn renderer() -> std::io::Result<PathBuf> {
    #[cfg(test)]
    if let Some(p) = std::env::var_os("PDF_EPSILON_RENDERER") {
        return Ok(p.into());
    }
    std::env::current_exe()
}

/// Renderer processes running, and the newest request of each kind.
static RUNNING: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());
static NEWEST: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
static TICKETS: AtomicU64 = AtomicU64::new(1);

/// A running slot, given back when dropped.
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        let (lock, wake) = &RUNNING;
        if let Ok(mut n) = lock.lock() {
            *n = n.saturating_sub(1);
        }
        wake.notify_all();
    }
}

/// Whether the request is still the newest of its kind.
fn wanted(kind: Option<&str>, ticket: u64) -> bool {
    let Some(kind) = kind else { return true };
    NEWEST.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(kind).copied())).is_none_or(|t| t == ticket)
}

/// Waits for a free slot, or gives up when the request is no longer wanted.
fn slot(kind: Option<&str>, ticket: u64) -> Option<Slot> {
    let (lock, wake) = &RUNNING;
    let mut n = lock.lock().ok()?;
    loop {
        if !wanted(kind, ticket) {
            return None;
        }
        if *n < AT_ONCE {
            *n += 1;
            return Some(Slot);
        }
        n = wake.wait_timeout(n, Duration::from_millis(100)).ok()?.0;
    }
}

/// Parent side: renders `pages` of `source` in a child process and calls `each` with every
/// image as it arrives (`None` for a page that could not be drawn). Pages not done when the
/// child dies or is killed are reported as `None`.
///
/// `kind` names the request's purpose ("thumbnails", say). A newer request of the same kind
/// cancels this one: it then stops, reports nothing more, and returns `false`.
pub fn pages(source: Source, pages: &[u32], width: u32, kind: Option<&str>, mut each: impl FnMut(u32, Option<Vec<u8>>)) -> std::io::Result<bool> {
    let ticket = TICKETS.fetch_add(1, Ordering::Relaxed);
    if let Some(k) = kind {
        if let Ok(mut m) = NEWEST.lock() {
            m.get_or_insert_with(HashMap::new).insert(k.to_string(), ticket);
        }
    }
    let Some(_slot) = slot(kind, ticket) else { return Ok(false) };
    let exe = renderer()?;
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
    let mut last = Instant::now();
    let mut done = std::collections::HashSet::new();
    let wanted_pages: std::collections::HashSet<u32> = pages.iter().copied().collect();
    let mut cancelled = false;
    while done.len() < wanted_pages.len() {
        if !wanted(kind, ticket) {
            cancelled = true;
            break;
        }
        let left = BATCH_TIMEOUT.saturating_sub(started.elapsed()).min(PAGE_TIMEOUT.saturating_sub(last.elapsed()));
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(100))) {
            Ok((page, png)) => {
                last = Instant::now();
                done.insert(page);
                each(page, (!png.is_empty()).then_some(png));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Gone: crashed, or ended by its own limits.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    if cancelled {
        return Ok(false);
    }
    for p in &wanted_pages {
        if !done.contains(p) {
            each(*p, None);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built app, which `cargo test` builds for the integration tests. `None` when absent.
    fn app() -> Option<PathBuf> {
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target.nosync").join(profile).join(format!("pdf-epsilon{}", std::env::consts::EXE_SUFFIX));
        p.exists().then_some(p)
    }

    /// One page whose forms draw each other ten times over ten levels: slow to render.
    fn slow_pdf(dir: &Path) -> PathBuf {
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 600] /Resources << /XObject << /F 5 0 R >> >> /Contents 4 0 R >>".into(),
            "<< /Length 6 >>\nstream\n/F Do\n\nendstream".into(),
        ];
        for i in 0..10 {
            let (res, body) = if i < 9 { (format!("/Resources << /XObject << /F {} 0 R >> >>", 6 + i), "/F Do ".repeat(10)) } else { (String::new(), "0 0 600 600 re f".into()) };
            objs.push(format!("<< /Type /XObject /Subtype /Form /BBox [0 0 600 600] {res} /Length {} >>\nstream\n{body}\nendstream", body.len()));
        }
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        let p = dir.join("slow.pdf");
        std::fs::write(&p, out).unwrap();
        p
    }

    // One test, so nothing else starts renderers while it counts them.
    #[test]
    fn at_most_two_renderers_run_and_a_newer_request_cancels_an_older_one() {
        let Some(app) = app() else {
            eprintln!("skipped: build the app first (cargo test builds it for tests/renderer.rs)");
            return;
        };
        std::env::set_var("PDF_EPSILON_RENDERER", app);
        let dir = std::env::temp_dir().join(format!("pdf-epsilon-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let slow = slow_pdf(&dir);

        // Four slow requests of different kinds: two run, two wait.
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let slow = slow.clone();
                std::thread::spawn(move || {
                    let started = Instant::now();
                    let r = pages(Source::File(slow), &[1], 64, Some(&format!("kind{i}")), |_, _| {});
                    (r.unwrap(), started.elapsed())
                })
            })
            .collect();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(*RUNNING.0.lock().unwrap(), 2);

        // A newer request of each kind cancels the older ones, running or waiting.
        for i in 0..4 {
            let n = NEWEST.lock().unwrap().as_mut().unwrap().get_mut(&format!("kind{i}")).map(|t| *t += 1_000_000);
            assert!(n.is_some());
        }
        for h in handles {
            let (finished, took) = h.join().unwrap();
            assert!(!finished);
            assert!(took < Duration::from_secs(8), "{took:?}");
        }
        assert_eq!(*RUNNING.0.lock().unwrap(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
