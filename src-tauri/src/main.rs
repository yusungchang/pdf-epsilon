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
        let ceiling = CEILING.load(Ordering::Relaxed);
        if before.saturating_add(size) > ceiling {
            // Only the renderer has a ceiling. Refusing the allocation would make Rust abort,
            // which writes a crash report and, inside the app bundle, may show a crash dialog.
            // Ending quietly is what the parent expects anyway.
            if ceiling != usize::MAX {
                render::quit(4);
            }
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

/// Reading the original: a file that is no longer there gets its own message.
fn reading(e: epsilon::Error) -> Failure {
    match &e {
        epsilon::Error::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Failure { code: "fileMissing", detail: String::new() },
        _ => Failure::from(e),
    }
}

/// Writing the copy: a folder that is gone, or that cannot be written to, gets its own message
/// (the system's own text would be in English whatever the interface language).
fn writing(e: std::io::Error) -> Failure {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => Failure { code: "folderMissing", detail: String::new() },
        PermissionDenied | ReadOnlyFilesystem => Failure { code: "notWritable", detail: String::new() },
        _ => Failure::from(e),
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
        epsilon::analyze_file_with_progress(&path, &report).map_err(reading)
    })
    .await
    .map_err(internal)?
}

/// Compressed copies made for comparison, kept so that saving writes exactly what was
/// compared. Keyed by file, settings and the file's size and modification time, so a file
/// changed on disk since is compressed again. Only the most recent few are kept, and at most
/// `PREPARED_BYTES` together (the newest always stays, so saving it still writes it).
#[derive(Default)]
struct Prepared(Mutex<Vec<PreparedCopy>>);

struct PreparedCopy {
    path: PathBuf,
    settings: String,
    stamp: Stamp,
    bytes: Arc<Vec<u8>>,
    report: Report,
}

const PREPARED_KEPT: usize = 3;
const PREPARED_BYTES: usize = 512 << 20;

fn settings_key(options: &Options) -> String {
    format!("{options:?}")
}

/// Size and modification time: what tells a file was changed or replaced.
type Stamp = Option<(u64, std::time::SystemTime)>;

fn stamp(path: &Path) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

impl Prepared {
    fn get(&self, path: &Path, options: &Options) -> Option<(Arc<Vec<u8>>, Report)> {
        let key = settings_key(options);
        let now = stamp(path);
        let mut list = self.0.lock().ok()?;
        list.retain(|c| c.path != path || (c.stamp.is_some() && c.stamp == now));
        list.iter().find(|c| c.path == path && c.settings == key).map(|c| (c.bytes.clone(), c.report.clone()))
    }

    fn put(&self, path: PathBuf, options: &Options, stamp: Stamp, bytes: Arc<Vec<u8>>, report: Report) {
        self.put_within(path, options, stamp, bytes, report, PREPARED_BYTES);
    }

    fn put_within(&self, path: PathBuf, options: &Options, stamp: Stamp, bytes: Arc<Vec<u8>>, report: Report, most: usize) {
        let Ok(mut list) = self.0.lock() else { return };
        let key = settings_key(options);
        list.retain(|c| !(c.path == path && c.settings == key));
        list.insert(0, PreparedCopy { path, settings: key, stamp, bytes, report });
        while list.len() > 1 && (list.len() > PREPARED_KEPT || list.iter().map(|c| c.bytes.len()).sum::<usize>() > most) {
            list.pop();
        }
    }
}

/// Makes the compressed copy for the comparison window, with progress from 0 to 1.
#[tauri::command]
async fn prepare(path: PathBuf, options: Options, progress: Channel<f32>, prepared: State<'_, Prepared>) -> Result<Report, Failure> {
    if let Some((_, report)) = prepared.get(&path, &options) {
        let _ = progress.send(1.0);
        return Ok(report);
    }
    // Stamped before reading, so a change while compressing makes the copy stale, not wrong.
    let before = stamp(&path);
    let (bytes, report) = {
        let path = path.clone();
        let options = options.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<(Vec<u8>, Report), Failure> {
            let input = epsilon::read_input(&path).map_err(reading)?;
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
    prepared.put(path, &options, before, Arc::new(bytes), report.clone());
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
/// process (see render.rs). Images are sent one by one as they are drawn. A newer request of
/// the same `kind` cancels this one, which then returns `false`.
#[tauri::command]
async fn render(
    path: PathBuf,
    options: Option<Options>,
    pages: Vec<u32>,
    width: u32,
    kind: Option<String>,
    each: Channel<PageImage>,
    prepared: State<'_, Prepared>,
) -> Result<bool, Failure> {
    let width = width.clamp(16, 4096);
    let pages: Vec<u32> = pages.into_iter().take(200).collect();
    let too_large = Failure { code: "tooLargeToShow", detail: String::new() };
    let source = match options {
        None => {
            if std::fs::metadata(&path).map(|m| m.len() > render::LARGEST).unwrap_or(false) {
                return Err(too_large);
            }
            render::Source::File(path)
        }
        Some(o) => match prepared.get(&path, &o) {
            Some((bytes, _)) if bytes.len() as u64 > render::LARGEST => return Err(too_large),
            Some((bytes, _)) => render::Source::Bytes(bytes),
            None => return Err(Failure { code: "notPrepared", detail: String::new() }),
        },
    };
    tauri::async_runtime::spawn_blocking(move || {
        render::pages(source, &pages, width, kind.as_deref(), |page, png| {
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
            let input = epsilon::read_input(&job.input).map_err(reading)?;
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
    let path = write_new(&dir, stem, &out).map_err(writing)?;
    carry_download_mark(&job.input, &path);
    Ok(Outcome { output: Some(path), report })
}

/// Longest file name written, in bytes: every common file system allows 255.
const NAME_BYTES: usize = 240;

/// `<stem>-epsilon.pdf`, or `-epsilon-<n>.pdf`, with the stem shortened to fit `NAME_BYTES`.
fn output_name(stem: &str, n: u32) -> String {
    let suffix = if n == 1 { "-epsilon.pdf".to_string() } else { format!("-epsilon-{n}.pdf") };
    let mut stem = stem;
    while stem.len() + suffix.len() > NAME_BYTES {
        let mut cut = stem.len() - 1;
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        stem = &stem[..cut];
    }
    format!("{stem}{suffix}")
}

/// Writes `<stem>-epsilon.pdf`, or `-epsilon-2`, `-3`, ... Never overwrites a file.
///
/// The data goes to a hidden temporary file first, which is then linked under the final name
/// (a link never replaces an existing file). So the final name never holds a half-written copy
/// that cloud sync might upload. On file systems without links, it is written in place.
fn write_new(dir: &Path, stem: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let temp = dir.join(format!(".pdf-epsilon-{}-{nanos}.tmp", std::process::id()));
    let mut f = OpenOptions::new().write(true).create_new(true).open(&temp)?;
    if let Err(e) = f.write_all(data).and_then(|_| f.sync_all()) {
        drop(f);
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    drop(f);
    for n in 1..10_000 {
        let path = dir.join(output_name(stem, n));
        match std::fs::hard_link(&temp, &path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&temp);
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                let _ = std::fs::remove_file(&temp);
                return write_in_place(dir, stem, data);
            }
        }
    }
    let _ = std::fs::remove_file(&temp);
    Err(std::io::Error::other("too many earlier outputs in this folder"))
}

fn write_in_place(dir: &Path, stem: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    for n in 1..10_000 {
        let path = dir.join(output_name(stem, n));
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

/// A file downloaded from the internet carries a mark that makes readers open it with care
/// (macOS quarantine, the Windows Mark of the Web). The smaller copy keeps the same content,
/// scripts included, so it keeps the mark too.
fn carry_download_mark(from: &Path, to: &Path) {
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let (Ok(from), Ok(to)) = (CString::new(from.as_os_str().as_bytes()), CString::new(to.as_os_str().as_bytes())) else { return };
        let name = c"com.apple.quarantine";
        let mut buf = vec![0u8; 4096];
        let n = unsafe { libc::getxattr(from.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
        if n > 0 {
            unsafe { libc::setxattr(to.as_ptr(), name.as_ptr(), buf.as_ptr().cast(), n as usize, 0, 0) };
        }
    }
    #[cfg(windows)]
    {
        let stream = |p: &Path| {
            let mut s = p.as_os_str().to_os_string();
            s.push(":Zone.Identifier");
            PathBuf::from(s)
        };
        if let Ok(mark) = std::fs::read(stream(from)) {
            let _ = std::fs::write(stream(to), mark);
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (from, to);
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
        .invoke_handler(tauri::generate_handler![presets, analyze, prepare, render, forget, compress])
        .run(tauri::generate_context!())
        .expect("error while running PDF Epsilon");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pdf-epsilon-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn long_names_are_shortened_to_fit_every_file_system() {
        assert_eq!(output_name("report", 1), "report-epsilon.pdf");
        assert_eq!(output_name("report", 3), "report-epsilon-3.pdf");
        // 100 Hangul syllables are 300 bytes: too long with the suffix on ext4 (255 bytes).
        let name = output_name(&"가".repeat(100), 12);
        assert!(name.len() <= NAME_BYTES && name.ends_with("-epsilon-12.pdf"), "{name}");
        assert!(name.starts_with("가가가"));
    }

    #[test]
    fn copies_are_written_whole_next_to_earlier_ones_and_never_over_them() {
        let dir = temp_dir("write");
        std::fs::write(dir.join("a-epsilon.pdf"), b"earlier").unwrap();
        let path = write_new(&dir, "a", b"new copy").unwrap();
        assert_eq!(path, dir.join("a-epsilon-2.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), b"new copy");
        assert_eq!(std::fs::read(dir.join("a-epsilon.pdf")).unwrap(), b"earlier");
        // No temporary file is left behind.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_errors_have_their_own_codes() {
        let gone = std::env::temp_dir().join("pdf-epsilon-no-such-folder").join("deeper");
        assert_eq!(write_new(&gone, "a", b"x").map_err(writing).unwrap_err().code, "folderMissing");
        let missing = epsilon::read_input(Path::new("/no/such/file.pdf")).map_err(reading).unwrap_err();
        assert_eq!(missing.code, "fileMissing");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = temp_dir("readonly");
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
            let result = write_new(&dir, "a", b"x").map_err(writing);
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            // Root can write anywhere; only check where the folder really is read-only.
            if let Err(e) = result {
                assert_eq!(e.code, "notWritable");
            }
        }
    }

    #[test]
    fn a_prepared_copy_is_dropped_when_its_file_changes() {
        let dir = temp_dir("stale");
        let file = dir.join("a.pdf");
        std::fs::write(&file, b"first version").unwrap();
        let prepared = Prepared::default();
        let options = Preset::Balanced.options();
        prepared.put(file.clone(), &options, stamp(&file), Arc::new(vec![1, 2, 3]), Report::default());
        assert!(prepared.get(&file, &options).is_some());
        std::fs::write(&file, b"second, longer version").unwrap();
        assert!(prepared.get(&file, &options).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_copies_stay_within_their_memory() {
        let prepared = Prepared::default();
        let options = Preset::Balanced.options();
        for name in ["a", "b", "c"] {
            prepared.put_within(PathBuf::from(name), &options, None, Arc::new(vec![0; 600]), Report::default(), 1000);
        }
        // Only the newest fits in 1000 bytes; it is kept even when alone it would not fit.
        let kept: Vec<PathBuf> = prepared.0.lock().unwrap().iter().map(|c| c.path.clone()).collect();
        assert_eq!(kept, vec![PathBuf::from("c")]);
        prepared.put_within(PathBuf::from("d"), &options, None, Arc::new(vec![0; 5000]), Report::default(), 1000);
        assert_eq!(prepared.0.lock().unwrap().len(), 1);
    }
}
