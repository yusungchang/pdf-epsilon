// No console window behind the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod render;

use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use epsilon::{Options, Preset, Report};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::State;

/// The system allocator, with an optional ceiling. The app runs without one; the thumbnail
/// renderer process sets one at start, so a hostile PDF that makes the renderer allocate
/// without end ends that process with an allocation failure instead of exhausting memory.
/// Works the same on every platform, unlike operating-system memory limits.
struct CappedAlloc;

static CEILING: AtomicUsize = AtomicUsize::new(usize::MAX);
static IN_USE: AtomicUsize = AtomicUsize::new(0);

impl CappedAlloc {
    fn claim(size: usize) -> bool {
        let before = IN_USE.fetch_add(size, Ordering::Relaxed);
        if before.saturating_add(size) > CEILING.load(Ordering::Relaxed) {
            IN_USE.fetch_sub(size, Ordering::Relaxed);
            return false;
        }
        true
    }
}

unsafe impl GlobalAlloc for CappedAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !Self::claim(layout.size()) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc(layout) };
        if p.is_null() {
            IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if !Self::claim(layout.size()) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.alloc_zeroed(layout) };
        if p.is_null() {
            IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let grow = new_size.saturating_sub(layout.size());
        if grow > 0 && !Self::claim(grow) {
            return std::ptr::null_mut();
        }
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if p.is_null() {
            IN_USE.fetch_sub(grow, Ordering::Relaxed);
        } else if new_size < layout.size() {
            IN_USE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: CappedAlloc = CappedAlloc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    input: PathBuf,
    /// `None` saves next to the original.
    output_dir: Option<PathBuf>,
    options: Options,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Outcome {
    /// `None` when no smaller file could be made and nothing was written.
    output: Option<PathBuf>,
    report: Report,
}

/// An error for the interface: a code it translates (`error.<code>` in ui/locales),
/// plus untranslated technical detail.
#[derive(Serialize)]
struct Failure {
    code: &'static str,
    detail: String,
}

impl From<epsilon::Error> for Failure {
    fn from(e: epsilon::Error) -> Self {
        Failure { code: e.code(), detail: e.detail() }
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure { code: "io", detail: e.to_string() }
    }
}

fn internal(e: impl std::fmt::Display) -> Failure {
    Failure { code: "internal", detail: e.to_string() }
}

#[derive(Serialize)]
struct PresetInfo {
    id: Preset,
    options: Options,
}

/// The engine's presets, so the interface shows the real settings.
#[tauri::command]
fn presets() -> Vec<PresetInfo> {
    [Preset::Lossless, Preset::Print, Preset::Balanced, Preset::Small]
        .into_iter()
        .map(|id| PresetInfo { id, options: id.options() })
        .collect()
}

#[tauri::command]
fn file_size(path: PathBuf) -> Result<u64, Failure> {
    Ok(std::fs::metadata(&path)?.len())
}

/// Explains where a file's bytes go and what each preset saves. Writes nothing.
/// Progress, from 0 to 1, goes to `progress` at most every 50 ms and on every whole percent.
#[tauri::command]
async fn analyze(path: PathBuf, progress: Channel<f32>) -> Result<epsilon::Analysis, Failure> {
    tauri::async_runtime::spawn_blocking(move || {
        let last = Mutex::new((Instant::now(), -1i32));
        let report = |done: f32| {
            let percent = (done * 100.0) as i32;
            let Ok(mut last) = last.lock() else { return };
            if percent > last.1 && (last.0.elapsed() >= Duration::from_millis(50) || percent - last.1 >= 1) {
                *last = (Instant::now(), percent);
                let _ = progress.send(done);
            }
        };
        epsilon::analyze_file_with_progress(&path, &report).map_err(Failure::from)
    })
    .await
    .map_err(internal)?
}

/// Compressed copies made for comparison, kept so that saving writes exactly what was
/// compared. Keyed by file and settings; only the most recent few are kept.
#[derive(Default)]
struct Prepared(Mutex<Vec<PreparedCopy>>);

struct PreparedCopy {
    path: PathBuf,
    settings: String,
    bytes: Arc<Vec<u8>>,
    report: Report,
}

const PREPARED_KEPT: usize = 3;

fn settings_key(options: &Options) -> String {
    format!("{options:?}")
}

impl Prepared {
    fn get(&self, path: &Path, options: &Options) -> Option<(Arc<Vec<u8>>, Report)> {
        let key = settings_key(options);
        let list = self.0.lock().ok()?;
        list.iter().find(|c| c.path == path && c.settings == key).map(|c| (c.bytes.clone(), c.report.clone()))
    }

    fn put(&self, path: PathBuf, options: &Options, bytes: Arc<Vec<u8>>, report: Report) {
        let Ok(mut list) = self.0.lock() else { return };
        let key = settings_key(options);
        list.retain(|c| !(c.path == path && c.settings == key));
        list.insert(0, PreparedCopy { path, settings: key, bytes, report });
        list.truncate(PREPARED_KEPT);
    }
}

/// Makes the compressed copy for the comparison window, with progress from 0 to 1.
#[tauri::command]
async fn prepare(path: PathBuf, options: Options, progress: Channel<f32>, prepared: State<'_, Prepared>) -> Result<Report, Failure> {
    if let Some((_, report)) = prepared.get(&path, &options) {
        let _ = progress.send(1.0);
        return Ok(report);
    }
    let (bytes, report) = {
        let path = path.clone();
        let options = options.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<u8>, Report), Failure> {
            let input = epsilon::read_input(&path)?;
            let last = Mutex::new(Instant::now());
            let report = |done: f32| {
                if let Ok(mut last) = last.lock() {
                    if last.elapsed() >= Duration::from_millis(50) || done >= 1.0 {
                        *last = Instant::now();
                        let _ = progress.send(done);
                    }
                }
            };
            Ok(epsilon::compress_bytes_with_progress(&input, &options, &report)?)
        })
        .await
        .map_err(internal)??
    };
    prepared.put(path, &options, Arc::new(bytes), report.clone());
    Ok(report)
}

/// Forgets the compressed copies of a file taken off the list.
#[tauri::command]
fn forget(path: PathBuf, prepared: State<'_, Prepared>) {
    if let Ok(mut list) = prepared.0.lock() {
        list.retain(|c| c.path != path);
    }
}

#[derive(Clone, Serialize)]
struct PageImage {
    page: u32,
    /// Base64 PNG, or `None` when the page could not be drawn.
    png: Option<String>,
}

/// Renders pages of a file, or with `options`, of its prepared compressed copy, in a separate
/// process (see render.rs). Images are sent one by one as they are drawn.
#[tauri::command]
async fn render(
    path: PathBuf,
    options: Option<Options>,
    pages: Vec<u32>,
    width: u32,
    each: Channel<PageImage>,
    prepared: State<'_, Prepared>,
) -> Result<(), Failure> {
    let width = width.clamp(16, 4096);
    let pages: Vec<u32> = pages.into_iter().take(200).collect();
    let source = match options {
        None => render::Source::File(path),
        Some(o) => match prepared.get(&path, &o) {
            Some((bytes, _)) => render::Source::Bytes(bytes),
            None => return Err(Failure { code: "notPrepared", detail: String::new() }),
        },
    };
    tauri::async_runtime::spawn_blocking(move || {
        render::pages(source, &pages, width, |page, png| {
            let _ = each.send(PageImage { page, png: png.map(|p| epsilon::to_base64(&p)) });
        })
        .map_err(Failure::from)
    })
    .await
    .map_err(internal)?
}

/// Saves the smaller copy. When it was prepared for comparison with the same settings, that
/// exact copy is written.
#[tauri::command]
async fn compress(job: Job, prepared: State<'_, Prepared>) -> Result<Outcome, Failure> {
    let ready = prepared.get(&job.input, &job.options);
    tauri::async_runtime::spawn_blocking(move || run(job, ready)).await.map_err(internal)?
}

fn run(job: Job, ready: Option<(Arc<Vec<u8>>, Report)>) -> Result<Outcome, Failure> {
    let (out, report) = match ready {
        Some((bytes, report)) => (bytes, report),
        None => {
            let input = epsilon::read_input(&job.input)?;
            let (out, report) = epsilon::compress_bytes(&input, &job.options)?;
            (Arc::new(out), report)
        }
    };
    if report.kept_original {
        return Ok(Outcome { output: None, report });
    }
    let dir = match job.output_dir {
        Some(d) => d,
        None => job.input.parent().map(Path::to_path_buf).unwrap_or_default(),
    };
    let stem = job.input.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
    let path = write_new(&dir, stem, &out)?;
    Ok(Outcome { output: Some(path), report })
}

/// Writes `<stem>-epsilon.pdf`, or `-epsilon-2`, `-3`, ... Never overwrites a file.
fn write_new(dir: &Path, stem: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    for n in 1..10_000 {
        let name = if n == 1 { format!("{stem}-epsilon.pdf") } else { format!("{stem}-epsilon-{n}.pdf") };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(data).and_then(|_| f.sync_all()) {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("too many earlier outputs in this folder"))
}

fn main() {
    // The thumbnail renderer: the same executable, started by the app (see render.rs).
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).map(|a| a == render::FLAG).unwrap_or(false) {
        CEILING.store(render::MEMORY, Ordering::Relaxed);
        let path = args.get(2).map(PathBuf::from).unwrap_or_default();
        let width = args.get(3).and_then(|w| w.to_str()?.parse().ok()).unwrap_or(160.0);
        std::process::exit(render::child(&path, width));
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Prepared::default())
        .invoke_handler(tauri::generate_handler![presets, file_size, analyze, prepare, render, forget, compress])
        .run(tauri::generate_context!())
        .expect("error while running PDF Epsilon");
}
