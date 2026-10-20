pub mod chat;
pub mod cut;
pub mod images;
pub mod job;
pub mod tools;
pub mod twitch;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};

/// FNV-1a: stable across runs and builds, unlike `DefaultHasher`.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

#[derive(Default)]
struct AppState {
    /// Set once every tool is found or installed.
    tools: Mutex<Option<tools::Tools>>,
    /// The running job; aborting it kills ffmpeg (see `cut::cut`).
    job: Mutex<Option<JoinHandle<()>>>,
}

/// Payload of `clip` events: one per state change or progress tick of a clip.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum ClipEvent {
    Running { index: usize, fraction: f64 },
    Done { index: usize, path: PathBuf },
    Failed { index: usize, error: String },
}

fn tools_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("tools"))
}

fn ready_tools(state: &AppState) -> Result<tools::Tools, String> {
    state.tools.lock().unwrap().clone().ok_or_else(|| "tools are not installed".to_owned())
}

#[tauri::command]
fn tools_status(app: AppHandle, state: State<AppState>) -> Result<Vec<tools::ToolInfo>, String> {
    let status = tools::status(&tools_dir(&app)?);
    *state.tools.lock().unwrap() = tools::Tools::from_status(&status);
    Ok(status)
}

/// Downloads missing tools, reporting `tools-progress` events; returns the new status.
#[tauri::command]
async fn tools_install(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<tools::ToolInfo>, String> {
    let dir = tools_dir(&app)?;
    let before = tools::status(&dir);
    let emit = |p: tools::Progress| {
        let _ = app.emit("tools-progress", p);
    };
    tools::install_missing(&dir, &before, &emit).await.map_err(|e| e.to_string())?;
    let status = tools::status(&dir);
    *state.tools.lock().unwrap() = tools::Tools::from_status(&status);
    Ok(status)
}

#[tauri::command]
async fn vod_info(input: String, state: State<'_, AppState>) -> Result<job::Vod, String> {
    let id = job::parse_vod_id(&input).ok_or_else(|| "not a Twitch VOD link".to_owned())?;
    cut::vod_info(&ready_tools(&state)?, &id).await.map_err(|e| e.to_string())
}

/// Starts cutting every interval in order, reporting `clip` events and `job-done`
/// at the end. `out_dir` defaults to `<Videos>/WClip`.
#[tauri::command]
fn job_start(
    app: AppHandle,
    state: State<AppState>,
    job: job::Job,
    out_dir: Option<PathBuf>,
) -> Result<(), String> {
    if let Some((i, e)) = job.check().into_iter().next() {
        return Err(format!("interval {}: {e}", i + 1));
    }
    let tools = ready_tools(&state)?;
    let out_dir = match out_dir {
        Some(dir) => dir,
        None => app.path().video_dir().map_err(|e| e.to_string())?.join("WClip"),
    };
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    let mut running = state.job.lock().unwrap();
    if running.as_ref().is_some_and(|h| !h.inner().is_finished()) {
        return Err("a job is already running".to_owned());
    }
    let task_app = app.clone();
    *running = Some(tauri::async_runtime::spawn(async move {
        let app = task_app;
        for (index, interval) in job.intervals.iter().enumerate() {
            let out = job::clip_path(&out_dir, &job.vod, interval);
            let progress = |fraction| {
                let _ = app.emit("clip", ClipEvent::Running { index, fraction });
            };
            let event = match cut::cut(&tools, &job.vod.playlist, interval, &out, progress).await {
                Ok(()) => ClipEvent::Done { index, path: out },
                Err(e) => ClipEvent::Failed { index, error: e.to_string() },
            };
            let _ = app.emit("clip", event);
        }
        let _ = app.emit("job-done", ());
    }));
    Ok(())
}

#[tauri::command]
fn job_cancel(state: State<AppState>) {
    if let Some(handle) = state.job.lock().unwrap().take() {
        handle.abort();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|app| {
            let dir = tools_dir(app.handle())?;
            tauri::async_runtime::spawn_blocking(move || tools::update_yt_dlp(&dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tools_status,
            tools_install,
            vod_info,
            job_start,
            job_cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
