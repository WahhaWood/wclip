pub mod chat;
pub mod cut;
pub mod encode;
pub mod errlog;
pub mod failure;
pub mod images;
pub mod job;
pub mod moments;
pub mod overlay;
pub mod settings;
pub mod tools;
pub mod twitch;
pub mod updater;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use failure::{Failure, Kind};
use serde::{Deserialize, Serialize};
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
    /// Recent preview frames: (playlist, VOD time, JPEG), newest last.
    preview_frames: Mutex<Vec<(String, f64, Arc<Vec<u8>>)>>,
    /// Chat of the whole video open on the home screen.
    chat: Mutex<Option<LoadedChat>>,
    /// The `chat_load` in progress; a new one aborts it.
    chat_task: Mutex<Option<tokio::task::AbortHandle>>,
    /// Working GPU encoders by codec, tried once per run (see `gpu_encoders`).
    gpus: tokio::sync::Mutex<HashMap<encode::Codec, Vec<encode::Encoder>>>,
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

/// Payload of `merge` events, after the clips when the job joins them.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
enum MergeEvent {
    Running,
    Done { path: PathBuf },
    Failed { error: Failure },
}

/// How `job_start` encodes and what chat it shows; the frontend passes the settings.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct JobOptions {
    output: encode::Output,
    chat_output: cut::ChatOutput,
    /// Also join the clips into one video.
    merge: bool,
    filter: chat::ChatFilter,
}

/// Chat of a whole video: for finding moments, and cut from without loading again.
struct LoadedChat {
    /// `chat_key` of the video and log it was loaded for.
    key: String,
    duration: f64,
    messages: Arc<Vec<chat::ChatMessage>>,
}

/// Identifies the chat of a VOD, or of a local video's log at a given sync.
fn chat_key(vod: &job::Vod, log: Option<&job::ChatLogRef>) -> Option<String> {
    match (vod.local, log) {
        (false, _) => Some(format!("vod:{}", vod.id)),
        (true, Some(log)) => Some(format!("log:{}@{}", log.path.display(), log.start)),
        (true, None) => None,
    }
}

/// Preview frames kept: two per interval while adjusting a few of them.
const PREVIEW_FRAMES: usize = 24;

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

/// Working GPU encoders for `codec`, tried on first use only (about a second).
async fn gpu_encoders(state: &AppState, tools: &tools::Tools, codec: encode::Codec) -> Vec<encode::Encoder> {
    let mut gpus = state.gpus.lock().await;
    if let Some(list) = gpus.get(&codec) {
        return list.clone();
    }
    let list = encode::available(tools, codec).await;
    gpus.insert(codec, list.clone());
    list
}

/// GPU encoders that work here for `codec`, for the settings screen.
#[tauri::command]
async fn encoders(state: State<'_, AppState>, codec: encode::Codec) -> Result<Vec<encode::Encoder>, String> {
    let tools = ready_tools(&state)?;
    Ok(gpu_encoders(&state, &tools, codec).await)
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

/// A video file on disk, to cut instead of a VOD.
#[tauri::command]
async fn local_video(path: PathBuf, state: State<'_, AppState>) -> Result<job::Vod, Failure> {
    cut::local_info(&ready_tools(&state)?, &path).await.map_err(|e| log("local video", Failure::new(e.kind(), e)))
}

/// Reads a chat log for a local video; the job reads it again when it starts.
#[tauri::command]
async fn chat_log_open(path: PathBuf) -> Result<chat::log::ChatLogInfo, Failure> {
    tauri::async_runtime::spawn_blocking(move || chat::log::open(&path))
        .await
        .map_err(|e| e.to_string())?
        .map(|log| log.info)
        .map_err(|e| log("chat log", Failure::new(Kind::Other, e)))
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

/// JPEG of the VOD frame at `time`, 960 px wide: behind the overlay preview and for
/// adjusting intervals. The last `PREVIEW_FRAMES` are kept.
#[tauri::command]
async fn preview_frame(state: State<'_, AppState>, playlist: String, time: f64) -> Result<Response, Failure> {
    if let Some((_, _, jpeg)) = state.preview_frames.lock().unwrap().iter().find(|(p, t, _)| *p == playlist && *t == time) {
        return Ok(Response::new(jpeg.to_vec()));
    }
    let tools = ready_tools(&state)?;
    let jpeg = cut::grab_frame(&tools, &playlist, time).await.map_err(|e| log("preview", Failure::new(e.kind(), e)))?;
    let mut frames = state.preview_frames.lock().unwrap();
    if frames.len() == PREVIEW_FRAMES {
        frames.remove(0);
    }
    frames.push((playlist, time, Arc::new(jpeg.clone())));
    Ok(Response::new(jpeg))
}

/// Loads the chat of the whole video, reporting `chat-progress` events (0..=1),
/// and keeps it for `chat_activity` and the clips; returns the message count. A
/// local video needs its `chat_log`. A newer call cancels this one, which then
/// fails with "cancelled".
#[tauri::command]
async fn chat_load(app: AppHandle, state: State<'_, AppState>, vod: job::Vod, chat_log: Option<job::ChatLogRef>) -> Result<usize, Failure> {
    let key = chat_key(&vod, chat_log.as_ref()).ok_or("no chat log")?;
    if let Some(loaded) = state.chat.lock().unwrap().as_ref().filter(|c| c.key == key) {
        return Ok(loaded.messages.len());
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(30)).build().map_err(|e| e.to_string())?;
    let task_app = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        match chat_log {
            Some(log_ref) if vod.local => {
                let path = log_ref.path.clone();
                let chat_log = tauri::async_runtime::spawn_blocking(move || chat::log::open(&path))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| Failure::new(Kind::Chat, e))?;
                Ok(chat::from_log(&client, &chat_log, log_ref.start).await)
            }
            _ => {
                let progress = |fraction: f64| {
                    let _ = task_app.emit("chat-progress", fraction);
                };
                chat::load(&client, &vod.id, 0.0, vod.duration, progress).await.map_err(|e| Failure::new(Kind::Chat, e))
            }
        }
    });
    if let Some(previous) = state.chat_task.lock().unwrap().replace(task.inner().abort_handle()) {
        previous.abort();
    }
    let messages = match task.await {
        Ok(result) => result.map_err(|e| log("chat load", e))?,
        Err(_) => return Err("cancelled".into()),
    };
    let count = messages.len();
    *state.chat.lock().unwrap() = Some(LoadedChat { key, duration: vod.duration, messages: Arc::new(messages) });
    Ok(count)
}

/// Activity of the loaded chat, messages matching `query` only (see
/// `moments::parse_query`).
#[tauri::command]
async fn chat_activity(state: State<'_, AppState>, query: String) -> Result<moments::Activity, String> {
    let (messages, duration) = {
        let chat = state.chat.lock().unwrap();
        let chat = chat.as_ref().ok_or("chat is not loaded")?;
        (chat.messages.clone(), chat.duration)
    };
    tauri::async_runtime::spawn_blocking(move || moments::activity(&messages, duration, &query))
        .await
        .map_err(|e| e.to_string())
}

/// PNG of the chat panel in `style` at 1080p with made-up messages; the settings
/// screen places it over the frame itself.
#[tauri::command]
async fn overlay_panel(style: overlay::OverlayStyle) -> Result<Response, String> {
    let png = tauri::async_runtime::spawn_blocking(move || {
        let mut out = std::io::Cursor::new(Vec::new());
        overlay::sample_panel(&style).write_to(&mut out, image::ImageFormat::Png).map(|()| out.into_inner())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(Response::new(png))
}

/// Starts cutting every interval in order, reporting `clip` events, `merge` events
/// when joining them and `job-done` at the end. No `overlay` style means clips
/// without chat; a local video without `chat_log` is cut without chat too.
/// `out_dir` defaults to `<Videos>/WClip`, `name_template` to `job::DEFAULT_TEMPLATE`.
#[tauri::command]
fn job_start(
    app: AppHandle,
    state: State<AppState>,
    job: job::Job,
    overlay: Option<overlay::OverlayStyle>,
    out_dir: Option<PathBuf>,
    name_template: Option<String>,
    options: Option<JobOptions>,
) -> Result<(), Failure> {
    let options = options.unwrap_or_default();
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

    // Chat already loaded for finding moments saves loading it per clip.
    let key = chat_key(&job.vod, job.chat_log.as_ref());
    let loaded = state.chat.lock().unwrap().as_ref().filter(|c| Some(&c.key) == key.as_ref()).map(|c| c.messages.clone());

    let mut running = state.job.lock().unwrap();
    if running.as_ref().is_some_and(|h| !h.inner().is_finished()) {
        return Err("a job is already running".into());
    }
    let task_app = app.clone();
    *running = Some(tauri::async_runtime::spawn(async move {
        let app = task_app;
        let cache = images::ImageCache::new(cache_dir, client.clone());
        // A local video's chat is read once for all its clips.
        let all_messages = match (loaded, &job.chat_log, &overlay) {
            (_, _, None) => None,
            (Some(all), _, _) => Some(Ok(all)),
            (None, Some(log_ref), _) if job.vod.local => {
                let path = log_ref.path.clone();
                let opened = tauri::async_runtime::spawn_blocking(move || chat::log::open(&path)).await;
                Some(match opened {
                    Ok(Ok(chat_log)) => Ok(Arc::new(chat::from_log(&client, &chat_log, log_ref.start).await)),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(e) => Err(e.to_string()),
                })
            }
            _ => None,
        };
        let overlay = overlay.filter(|_| !job.vod.local || all_messages.is_some());
        let mut video = match options.output.encoder {
            encode::Encoder::Auto => {
                let gpus = gpu_encoders(&app.state::<AppState>(), &tools, options.output.codec).await;
                encode::resolve(options.output, &gpus)
            }
            _ => options.output,
        };
        let mut done = Vec::new();
        for (index, interval) in job.intervals.iter().enumerate() {
            let _ = app.emit("clip", ClipEvent::Preparing { index });
            let chat = match &overlay {
                Some(style) => match &all_messages {
                    Some(Ok(all)) => Ok(Some(cut::chat_from(&cache, &job.vod, interval, style, all).await)),
                    Some(Err(e)) => Err(e.clone()),
                    None => cut::load_chat(&client, &cache, &job.vod, interval, style).await.map(Some).map_err(|e| e.to_string()),
                },
                None => Ok(None),
            };
            let chat = match chat {
                Ok(chat) => chat,
                Err(e) => {
                    let _ = app.emit("clip", ClipEvent::Failed { index, error: log(&format!("clip {} chat", index + 1), Failure::new(Kind::Chat, e)) });
                    continue;
                }
            };
            let chat = chat.map(|mut c| {
                c.messages.retain(|m| options.filter.keeps(m));
                c
            });
            // An empty panel would only cover the video.
            let no_chat = chat.as_ref().is_some_and(|c| c.messages.is_empty());
            let chat = chat.filter(|c| !c.messages.is_empty());
            let out = job::clip_path(&out_dir, &template, &job.vod, interval, index + 1);
            let progress = |fraction| {
                let _ = app.emit("clip", ClipEvent::Running { index, fraction });
            };
            let mut attempt = 0;
            let event = loop {
                let cut_options = cut::CutOptions { video, chat: options.chat_output };
                match cut::cut(&tools, &job.vod.playlist, interval, chat.clone(), cut_options, &out, progress).await {
                    Ok(()) => {
                        done.push((out.clone(), interval.clone()));
                        break ClipEvent::Done { index, path: out, no_chat };
                    }
                    Err(e) if e.kind() == Kind::Network && attempt < RETRY_DELAYS.len() => {
                        errlog::write(&format!("clip {} retry {}", index + 1, attempt + 1), &e.to_string());
                        let _ = app.emit("clip", ClipEvent::Retrying { index, attempt: attempt + 1 });
                        tokio::time::sleep(RETRY_DELAYS[attempt]).await;
                        attempt += 1;
                    }
                    // A GPU encoder can fail on a driver or a frame format; the CPU
                    // always works, so the rest of the job goes there too.
                    Err(e) if video.encoder != encode::Encoder::Cpu && e.kind() != Kind::DiskFull => {
                        errlog::write(&format!("clip {} on {:?}, now on the CPU", index + 1, video.encoder), &e.to_string());
                        video = video.on_cpu();
                    }
                    Err(e) => {
                        let error = log(&format!("clip {}", index + 1), Failure::new(e.kind(), e));
                        break ClipEvent::Failed { index, error };
                    }
                }
            };
            let _ = app.emit("clip", event);
        }
        if options.merge && done.len() >= 2 {
            let _ = app.emit("merge", MergeEvent::Running);
            let (clips, intervals): (Vec<PathBuf>, Vec<job::Interval>) = done.into_iter().unzip();
            let out = job::merged_path(&out_dir, &template, &job.vod, &intervals);
            let event = match cut::concat(&tools, &clips, &out).await {
                Ok(()) => MergeEvent::Done { path: out },
                Err(e) => MergeEvent::Failed { error: log("merge", Failure::new(e.kind(), e)) },
            };
            let _ = app.emit("merge", event);
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
            tauri::async_runtime::spawn_blocking(overlay::text::Typesetter::warm_up);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            tools_status,
            tools_install,
            vod_info,
            local_video,
            chat_log_open,
            chat_load,
            chat_activity,
            encoders,
            settings_load,
            settings_save,
            font_families,
            preview_frame,
            overlay_panel,
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
