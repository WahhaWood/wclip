pub mod chat;
pub mod cut;
pub mod images;
pub mod job;
pub mod overlay;
pub mod settings;
pub mod tools;
pub mod twitch;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use image::RgbaImage;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::Response;
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
    /// Last preview frame: (playlist, VOD time, frame).
    preview_frame: Mutex<Option<(String, f64, Arc<RgbaImage>)>>,
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

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("settings.json"))
}

fn default_out_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().video_dir().map_err(|e| e.to_string())?.join("WClip"))
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

/// Settings with `out_dir` filled in when the user has not picked one.
#[tauri::command]
fn settings_load(app: AppHandle) -> Result<settings::Settings, String> {
    let mut s = settings::load(&settings_path(&app)?);
    if s.out_dir.is_none() {
        s.out_dir = Some(default_out_dir(&app)?);
    }
    Ok(s)
}

#[tauri::command]
fn settings_save(app: AppHandle, settings: settings::Settings) -> Result<(), String> {
    settings::save(&settings_path(&app)?, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn font_families() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| overlay::text::Typesetter::new().families())
        .await
        .map_err(|e| e.to_string())
}

/// PNG of the VOD frame at `time` with the overlay in `style` over it, 960 px wide.
/// The frame is fetched once per (playlist, time); without a playlist the overlay
/// goes over a plain 1080p background.
#[tauri::command]
async fn overlay_preview(
    state: State<'_, AppState>,
    playlist: Option<String>,
    time: f64,
    style: overlay::OverlayStyle,
) -> Result<Response, String> {
    let cached = state.preview_frame.lock().unwrap().clone();
    let frame = match (cached, playlist) {
        (_, None) => Arc::new(RgbaImage::from_fn(1920, 1080, |x, y| {
            let v = (40 + (x + y) * 50 / 3000) as u8;
            image::Rgba([v / 2, v / 2, v, 255])
        })),
        (Some((p, t, frame)), Some(playlist)) if p == playlist && t == time => frame,
        (_, Some(playlist)) => {
            let tools = ready_tools(&state)?;
            let frame = Arc::new(cut::grab_frame(&tools, &playlist, time).await.map_err(|e| e.to_string())?);
            *state.preview_frame.lock().unwrap() = Some((playlist, time, Arc::clone(&frame)));
            frame
        }
    };
    let png = tauri::async_runtime::spawn_blocking(move || {
        let img = overlay::preview(&frame, &style);
        let height = (img.height() as f32 * 960.0 / img.width() as f32).round() as u32;
        let small = image::imageops::resize(&img, 960, height.max(1), image::imageops::FilterType::Triangle);
        let mut out = std::io::Cursor::new(Vec::new());
        small.write_to(&mut out, image::ImageFormat::Png).map(|()| out.into_inner())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(Response::new(png))
}

/// Starts cutting every interval in order, reporting `clip` events and `job-done`
/// at the end. No `overlay` style means clips without chat. `out_dir` defaults to
/// `<Videos>/WClip`, `name_template` to `job::DEFAULT_TEMPLATE`.
#[tauri::command]
fn job_start(
    app: AppHandle,
    state: State<AppState>,
    job: job::Job,
    overlay: Option<overlay::OverlayStyle>,
    out_dir: Option<PathBuf>,
    name_template: Option<String>,
) -> Result<(), String> {
    if let Some((i, e)) = job.check().into_iter().next() {
        return Err(format!("interval {}: {e}", i + 1));
    }
    let tools = ready_tools(&state)?;
    let out_dir = match out_dir {
        Some(dir) => dir,
        None => default_out_dir(&app)?,
    };
    let template = name_template.unwrap_or_default();
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("images");

    let mut running = state.job.lock().unwrap();
    if running.as_ref().is_some_and(|h| !h.inner().is_finished()) {
        return Err("a job is already running".to_owned());
    }
    let task_app = app.clone();
    *running = Some(tauri::async_runtime::spawn(async move {
        let app = task_app;
        let client = reqwest::Client::new();
        let cache = images::ImageCache::new(cache_dir, client.clone());
        for (index, interval) in job.intervals.iter().enumerate() {
            let _ = app.emit("clip", ClipEvent::Running { index, fraction: 0.0 });
            let result = async {
                let chat = match &overlay {
                    Some(style) => Some(
                        cut::load_chat(&client, &cache, &job.vod, interval, style)
                            .await
                            .map_err(|e| e.to_string())?,
                    ),
                    None => None,
                };
                let out = job::clip_path(&out_dir, &template, &job.vod, interval, index + 1);
                let progress = |fraction| {
                    let _ = app.emit("clip", ClipEvent::Running { index, fraction });
                };
                cut::cut(&tools, &job.vod.playlist, interval, chat, &out, progress)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(out)
            };
            let event = match result.await {
                Ok(path) => ClipEvent::Done { index, path },
                Err(error) => ClipEvent::Failed { index, error },
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
        .plugin(tauri_plugin_dialog::init())
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
            settings_load,
            settings_save,
            font_families,
            overlay_preview,
            job_start,
            job_cancel
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
