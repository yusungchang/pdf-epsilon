// No console window behind the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use epsilon::{Options, Preset, Report};
use serde::{Deserialize, Serialize};

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
#[tauri::command]
async fn analyze(path: PathBuf) -> Result<epsilon::Analysis, Failure> {
    tauri::async_runtime::spawn_blocking(move || epsilon::analyze_file(&path).map_err(Failure::from))
        .await
        .map_err(internal)?
}

#[tauri::command]
async fn compress(job: Job) -> Result<Outcome, Failure> {
    tauri::async_runtime::spawn_blocking(move || run(job)).await.map_err(internal)?
}

fn run(job: Job) -> Result<Outcome, Failure> {
    let bytes = epsilon::read_input(&job.input)?;
    let (out, report) = epsilon::compress_bytes(&bytes, &job.options)?;
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![presets, file_size, analyze, compress])
        .run(tauri::generate_context!())
        .expect("error while running PDF Epsilon");
}
