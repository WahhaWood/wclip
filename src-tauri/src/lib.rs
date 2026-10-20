pub mod chat;
pub mod cut;
pub mod errlog;
pub mod failure;
pub mod images;
pub mod job;
pub mod overlay;
pub mod settings;
pub mod tools;
pub mod twitch;
pub mod updater;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use failure::{Failure, Kind};
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
    /// Release found by the last update check.
    update: Mutex<Option<updater::Release>>,
    /// Updated copy to start on restart.
    update_ready: Mutex<Option<PathBuf>>,
}

/// Payload of `clip` events: one per state change or progress tick of a clip.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case", rename_all_fields = "camelCase")]
enum ClipEvent {
    /// Loading the chat and its images.
    Preparing { index: usize },
    Running { index: usize, fraction: f64 },
    /// The connection dropped; the clip starts over after a pause.
    Retrying { index: usize, attempt: usize },
    /// `no_chat`: nobody wrote during the clip, so it was cut without the panel.
    Done { index: usize, path: PathBuf, no_chat: bool },
    Failed { index: usize, error: Failure },
}

/// Pauses before cutting a clip again after a network failure; one retry each.
const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(5), Duration::from_secs(15)];

fn tools_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_data_dir().map_err(|e| e.to_string())?.join("tools"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_config_dir().map_err(|e| e.to_string())?.join("settings.json"))
}

/// `<Videos>/WClip`. Without an XDG videos dir (common on Linux) it falls back to
/// Downloads, then `~/Videos`.
fn default_out_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let p = app.path();
    let base = p
        .video_dir()
        .or_else(|_| p.download_dir())
        .or_else(|_| p.home_dir().map(|h| h.join("Videos")))
        .map_err(|e| e.to_string())?;
    Ok(base.join("WClip"))
}

/// Writes `failure` to the error log on its way to the UI.
fn log(context: &str, failure: Failure) -> Failure {
    errlog::write(context, &failure.detail);
    failure
}

fn job_running(state: &AppState) -> bool {
    state.job.lock().unwrap().as_ref().is_some_and(|h| !h.inner().is_finished())
}

/// No total timeout: a release is ~80 MB. A stalled read still fails.
fn updater_client() -> Result<reqwest::Client, Failure> {
    let builder = reqwest::Client::builder().connect_timeout(Duration::from_secs(15)).read_timeout(Duration::from_secs(30));
    Ok(builder.build().map_err(|e| e.to_string())?)
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
    tools::install_missing(&dir, &before, &emit).await.map_err(|e| log("tools", e.to_string().into()).detail)?;
    let status = tools::status(&dir);
    *state.tools.lock().unwrap() = tools::Tools::from_status(&status);
    Ok(status)
}

#[tauri::command]
async fn vod_info(input: String, state: State<'_, AppState>) -> Result<job::Vod, Failure> {
    let id = job::parse_vod_id(&input).ok_or("not a Twitch VOD link")?;
    cut::vod_info(&ready_tools(&state)?, &id).await.map_err(|e| log("vod", Failure::new(e.kind(), e)))
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
) -> Result<Response, Failure> {
    let cached = state.preview_frame.lock().unwrap().clone();
    let frame = match (cached, playlist) {
        (_, None) => Arc::new(RgbaImage::from_fn(1920, 1080, |x, y| {
            let v = (40 + (x + y) * 50 / 3000) as u8;
            image::Rgba([v / 2, v / 2, v, 255])
        })),
        (Some((p, t, frame)), Some(playlist)) if p == playlist && t == time => frame,
        (_, Some(playlist)) => {
            let tools = ready_tools(&state)?;
            let frame = Arc::new(cut::grab_frame(&tools, &playlist, time).await.map_err(|e| log("preview", Failure::new(e.kind(), e)))?);
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
) -> Result<(), Failure> {
    if let Some((i, e)) = job.check().into_iter().next() {
        return Err(format!("interval {}: {e}", i + 1).into());
    }
    let tools = ready_tools(&state)?;
    let out_dir = match out_dir {
        Some(dir) => dir,
        None => default_out_dir(&app)?,
    };
    let template = name_template.unwrap_or_default();
    std::fs::create_dir_all(&out_dir).map_err(|e| log("out dir", e.into()))?;
    let cache_dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("images");
    // Without a timeout a request on a dropped connection can hang forever.
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;

    let mut running = state.job.lock().unwrap();
    if running.as_ref().is_some_and(|h| !h.inner().is_finished()) {
        return Err("a job is already running".into());
    }
    let task_app = app.clone();
    *running = Some(tauri::async_runtime::spawn(async move {
        let app = task_app;
        let cache = images::ImageCache::new(cache_dir, client.clone());
        for (index, interval) in job.intervals.iter().enumerate() {
            let _ = app.emit("clip", ClipEvent::Preparing { index });
            let chat = match &overlay {
                Some(style) => match cut::load_chat(&client, &cache, &job.vod, interval, style).await {
                    Ok(chat) => Some(chat),
                    Err(e) => {
                        let _ = app.emit("clip", ClipEvent::Failed { index, error: log(&format!("clip {} chat", index + 1), Failure::new(Kind::Chat, e)) });
                        continue;
                    }
                },
                None => None,
            };
            // An empty panel would only cover the video.
            let no_chat = chat.as_ref().is_some_and(|c| c.messages.is_empty());
            let chat = chat.filter(|c| !c.messages.is_empty());
            let out = job::clip_path(&out_dir, &template, &job.vod, interval, index + 1);
            let progress = |fraction| {
                let _ = app.emit("clip", ClipEvent::Running { index, fraction });
            };
            let mut attempt = 0;
            let event = loop {
                match cut::cut(&tools, &job.vod.playlist, interval, chat.clone(), &out, progress).await {
                    Ok(()) => break ClipEvent::Done { index, path: out, no_chat },
                    Err(e) if e.kind() == Kind::Network && attempt < RETRY_DELAYS.len() => {
                        errlog::write(&format!("clip {} retry {}", index + 1, attempt + 1), &e.to_string());
                        let _ = app.emit("clip", ClipEvent::Retrying { index, attempt: attempt + 1 });
                        tokio::time::sleep(RETRY_DELAYS[attempt]).await;
                        attempt += 1;
                    }
                    Err(e) => {
                        let error = log(&format!("clip {}", index + 1), Failure::new(e.kind(), e));
                        break ClipEvent::Failed { index, error };
                    }
                }
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

/// The latest release if it is newer, checked at most once a day unless `force`.
/// A failed unforced check stays silent: being offline is no reason to bother the user.
#[tauri::command]
async fn update_check(app: AppHandle, state: State<'_, AppState>, force: bool) -> Result<Option<updater::Info>, Failure> {
    let stamp = app.path().app_data_dir().map_err(|e| e.to_string())?.join("update.checked");
    if !force && !tools::update_due(&stamp, SystemTime::now()) {
        return Ok(None);
    }
    match updater::latest(&updater_client()?).await {
        Ok(release) => {
            if let Some(dir) = stamp.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&stamp, b"");
            let info = release.as_ref().map(updater::Release::info);
            *state.update.lock().unwrap() = release;
            Ok(info)
        }
        Err(e) => {
            let failure = log("update check", Failure::new(e.kind(), e));
            if force { Err(failure) } else { Ok(None) }
        }
    }
}

/// Downloads and puts in place the release found by `update_check`, reporting
/// `update-progress` events; `update_restart` then starts it.
#[tauri::command]
async fn update_install(app: AppHandle, state: State<'_, AppState>) -> Result<(), Failure> {
    let release = state.update.lock().unwrap().clone().ok_or("no update found")?;
    let progress = |done: u64, total: Option<u64>| {
        let _ = app.emit("update-progress", serde_json::json!({ "done": done, "total": total }));
    };
    let target = updater::target().ok_or("this copy cannot update itself")?;
    updater::install(&updater_client()?, &release, &target, progress)
        .await
        .map_err(|e| log("update", Failure::new(e.kind(), e)))?;
    *state.update_ready.lock().unwrap() = Some(target);
    Ok(())
}

/// Starts the updated copy and quits; refused while clips are being cut.
#[tauri::command]
fn update_restart(app: AppHandle, state: State<AppState>) -> Result<(), Failure> {
    if job_running(&state) {
        return Err("clips are still being cut".into());
    }
    let target = state.update_ready.lock().unwrap().clone().ok_or("no update installed")?;
    updater::launch(&target).map_err(|e| log("restart", e.into()))?;
    app.exit(0);
    Ok(())
}

/// The error log file, `None` while it is empty.
#[tauri::command]
fn error_log() -> Option<PathBuf> {
    errlog::path().map(PathBuf::from)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            errlog::init(app.path().app_log_dir()?.join("wclip.log"));
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
            job_cancel,
            error_log,
            update_check,
            update_install,
            update_restart
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
