//! Cutting clips: yt-dlp resolves the VOD's HLS playlist, ffmpeg reads it directly.
//!
//! Input seeking (`-ss` before `-i`) on Twitch HLS is frame-accurate when re-encoding
//! (checked 2026-10-20), so frame 0 of a clip is exactly the interval start and no
//! temporary download is needed.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::chat::{self, ChatMessage, Part};
use crate::encode::Output;
use crate::failure::{self, Kind};
use crate::images::{ImageCache, Images};
use crate::job::{Interval, Vod};
use crate::layout::Layout;
use crate::overlay::{self, Overlay, OverlayStyle};
use crate::tools::{self, Tools};

/// HLS input options: give up on a stalled read after 20 s, retry a failed
/// segment 3 times before the demuxer skips it.
const HLS_INPUT: [&str; 4] = ["-rw_timeout", "20000000", "-seg_max_retry", "3"];

/// Input options for `input`: a VOD playlist URL or a local file path.
fn input_args(input: &str) -> &'static [&'static str] {
    if input.starts_with("http") { &HLS_INPUT } else { &[] }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Tool(String),
    #[error("yt-dlp output: {0}")]
    Json(#[from] serde_json::Error),
    /// The HLS demuxer skips a segment it cannot download and carries on (exit
    /// code 0), which would leave a hole in the clip.
    #[error("ffmpeg: {0}")]
    SegmentSkipped(String),
    /// Not a cookie value: only letters and digits fit the cookie file.
    #[error("the Twitch token is not a cookie value")]
    BadToken,
}

impl Error {
    pub fn kind(&self) -> Kind {
        match self {
            Error::Io(e) => failure::io_kind(e),
            Error::Tool(text) => failure::classify(text),
            Error::Json(_) => Kind::Other,
            Error::SegmentSkipped(_) => Kind::Network,
            Error::BadToken => Kind::BadToken,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Deserialize)]
struct YtDlpInfo {
    title: String,
    uploader: Option<String>,
    duration: f64,
    url: String,
    thumbnail: Option<String>,
    height: Option<u32>,
}

/// A subscriber-only VOD is asked for again with `token`, the `auth-token` cookie
/// of twitch.tv, when there is one. Not before: Twitch refuses even public VODs to
/// a stale token.
pub async fn vod_info(tools: &Tools, id: &str, token: Option<&str>) -> Result<Vod> {
    match (yt_dlp_info(tools, id, None).await, token) {
        (Err(e), Some(token)) if e.kind() == Kind::SubOnly => yt_dlp_info(tools, id, Some(token)).await,
        (result, _) => result,
    }
}

async fn yt_dlp_info(tools: &Tools, id: &str, token: Option<&str>) -> Result<Vod> {
    let mut command = tokio_command(&tools.yt_dlp);
    command.args(["-J", "-f", "b", "--no-warnings", "--no-playlist"]);
    // Removed when this returns.
    let cookies = token.map(CookieFile::new).transpose()?;
    if let Some(cookies) = &cookies {
        command.arg("--cookies").arg(&cookies.0);
        command.arg("--extractor-args").arg(format!("twitch:client_id={}", crate::twitch::CLIENT_ID));
    }
    let out = command.arg(format!("https://www.twitch.tv/videos/{id}")).output().await?;
    if !out.status.success() {
        return Err(tool_error(&out.stderr, "yt-dlp"));
    }
    let info: YtDlpInfo = serde_json::from_slice(&out.stdout)?;
    Ok(Vod {
        id: id.to_owned(),
        title: info.title,
        channel: info.uploader.unwrap_or_default(),
        duration: info.duration,
        thumbnail: info.thumbnail,
        playlist: info.url,
        height: info.height,
        local: false,
        started: None,
    })
}

/// The twitch.tv login as a Netscape cookie file for yt-dlp, in the temp dir and
/// readable by the user only; removed on drop.
struct CookieFile(PathBuf);

impl CookieFile {
    fn new(token: &str) -> Result<Self> {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let token = token.trim();
        if token.is_empty() || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::BadToken);
        }
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("wclip-cookies-{}-{n}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut file = crate::private_file(&path)?;
        let cookies = Self(path);
        // Expires in 2100: yt-dlp may drop session cookies.
        write!(file, "# Netscape HTTP Cookie File\n.twitch.tv\tTRUE\t/\tTRUE\t4102444800\tauth-token\t{token}\n")?;
        Ok(cookies)
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Deserialize)]
struct Probe {
    format: ProbeFormat,
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbeStream {
    height: Option<u32>,
}

/// A video file on disk as a `Vod` (see `Vod::local`).
pub async fn local_info(tools: &Tools, path: &Path) -> Result<Vod> {
    let out = tokio_command(&tools.ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_entries", "format=duration:stream=height"])
        .args(["-select_streams", "v:0"])
        .arg(path)
        .output()
        .await?;
    if !out.status.success() {
        return Err(tool_error(&out.stderr, "ffprobe"));
    }
    let probe: Probe = serde_json::from_slice(&out.stdout)?;
    let duration = probe.format.duration.and_then(|d| d.parse::<f64>().ok());
    let (Some(duration), Some(height)) = (duration, probe.streams.first().map(|s| s.height)) else {
        return Err(Error::Tool("ffprobe: no video in the file".into()));
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Vod {
        started: chat::log::started_from_name(&stem, duration),
        id: stem,
        title: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        channel: String::new(),
        duration,
        thumbnail: None,
        playlist: path.to_string_lossy().into_owned(),
        height,
        local: true,
    })
}

/// Chat to burn into a clip.
#[derive(Clone)]
pub struct ChatOverlay {
    /// Already scaled to the video height.
    pub style: OverlayStyle,
    pub messages: Vec<ChatMessage>,
    pub images: Images,
}

/// JPEG of the video frame at VOD time `t`, 960 px wide, for the overlay preview.
pub async fn grab_frame(tools: &Tools, playlist: &str, t: f64) -> Result<Vec<u8>> {
    let out = tokio_command(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .args(input_args(playlist))
        .args(["-ss", &t.to_string(), "-i", playlist])
        .args(["-frames:v", "1", "-vf", "scale=960:-2", "-q:v", "3", "-f", "image2pipe", "-c:v", "mjpeg", "pipe:1"])
        .output()
        .await?;
    if !out.status.success() || out.stdout.is_empty() {
        return Err(tool_error(&out.stderr, "ffmpeg"));
    }
    Ok(out.stdout)
}

/// The chat style in pixels of the clip: scaled to the video height, or placed
/// on the canvas of a vertical clip.
pub fn clip_style(style: &OverlayStyle, vod: &Vod, layout: Option<&Layout>) -> OverlayStyle {
    match layout {
        Some(layout) => {
            let b = layout.chat;
            let even = |v: u32| v.max(2) & !1;
            OverlayStyle { x: b.x, y: b.y, width: even(b.width), height: even(b.height), font_size: b.font_size, ..style.clone() }
        }
        None => style.scaled(vod.height.unwrap_or(1080)),
    }
}

/// Loads the chat shown during `interval` (with lookback, so the panel starts
/// filled) and every badge and emote image in it. Images that fail to load are
/// left out; the renderer falls back to text for them. `style` is from `clip_style`.
pub async fn load_chat(
    client: &reqwest::Client,
    cache: &ImageCache,
    vod: &Vod,
    interval: &Interval,
    style: &OverlayStyle,
) -> crate::twitch::Result<ChatOverlay> {
    let style = style.clone();
    let start = (interval.start - style.lookback()).max(0.0);
    let messages = chat::load(client, &vod.id, start, interval.end, |_| {}).await?;
    Ok(with_images(cache, style, messages).await)
}

/// Like `load_chat`, from messages already loaded: a chat log (`chat::from_log`) or
/// the whole VOD's chat. Lookback may reach before the video: the chat went on
/// before the recording started.
pub async fn chat_from(cache: &ImageCache, interval: &Interval, style: &OverlayStyle, all: &[ChatMessage]) -> ChatOverlay {
    let style = style.clone();
    let start = interval.start - style.lookback();
    let messages = all.iter().filter(|m| m.time >= start && m.time < interval.end).cloned().collect();
    with_images(cache, style, messages).await
}

async fn with_images(cache: &ImageCache, style: OverlayStyle, messages: Vec<ChatMessage>) -> ChatOverlay {
    let urls: BTreeSet<String> = messages
        .iter()
        .flat_map(|m| {
            let emotes = m.parts.iter().filter_map(|p| match p {
                Part::Emote { url, .. } => Some(url),
                Part::Text(_) => None,
            });
            m.badges.iter().chain(emotes).cloned()
        })
        .collect();
    let images = cache.get_all(urls).await;
    ChatOverlay { style, messages, images }
}

/// Where the chat of a clip goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatOutput {
    /// Into the video.
    #[default]
    Burn,
    /// A separate video with transparency next to the clip (see `chat_path`), for editing.
    File,
    Both,
}

#[derive(Debug, Clone, Default)]
pub struct CutOptions {
    /// `Encoder::Auto` encodes on the CPU; resolve it first (`encode::resolve`).
    pub video: Output,
    pub chat: ChatOutput,
    /// A vertical clip laid out so; the chat style is then in canvas pixels.
    pub layout: Option<Layout>,
}

/// The chat of clip `out` as its own video: ProRes 4444 keeps the transparency and
/// every editor reads it.
pub fn chat_path(out: &Path) -> PathBuf {
    out.with_extension("chat.mov")
}

/// Encodes `interval` of the VOD into `out` (MP4), with the chat on top and/or
/// next to it (`options.chat`) if given. `on_progress` gets 0..=1. Dropping the
/// future kills ffmpeg and removes the partial files.
pub async fn cut(
    tools: &Tools,
    playlist: &str,
    interval: &Interval,
    chat: Option<ChatOverlay>,
    options: CutOptions,
    out: &Path,
    on_progress: impl Fn(f64),
) -> Result<()> {
    let length = interval.end - interval.start;
    let video = options.video.args();
    let burn = chat.is_some() && options.chat != ChatOutput::File;
    let chat_part = (chat.is_some() && options.chat != ChatOutput::Burn).then(|| PartFile(chat_path(out).with_extension("mov.part")));
    let part = PartFile(out.with_extension("mp4.part"));
    let mut cmd = tokio_command(&tools.ffmpeg);
    // Warnings too: that is where the demuxer reports a skipped segment.
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "level+warning", "-nostats", "-progress", "pipe:1"])
        .args(&video.global)
        .args(input_args(playlist))
        .args(["-ss", &interval.start.to_string(), "-i", playlist]);
    let (layout, main) = match &options.layout {
        Some(layout) => (format!("{};", layout.graph("main")), "[main]"),
        None => (String::new(), "[0:v]"),
    };
    let graph = match &chat {
        Some(chat) => {
            let (w, h) = (chat.style.width, chat.style.height);
            let (x, y) = chat.style.position();
            cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}")])
                .args(["-framerate", &overlay::FPS.to_string(), "-i", "pipe:0"])
                .stdin(Stdio::piped());
            if burn {
                format!("{layout}{main}[1:v]overlay=x={x}:y={y}:eof_action=repeat,{}[v]", video.filter)
            } else {
                format!("{layout}{main}{}[v]", video.filter)
            }
        }
        None => {
            cmd.stdin(Stdio::null());
            format!("{layout}{main}{}[v]", video.filter)
        }
    };
    cmd.args(["-filter_complex", &graph, "-map", "[v]", "-map", "0:a:0?", "-t", &length.to_string()])
        .args(&video.codec)
        .args(["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", "-f", "mp4", "-y"])
        .arg(&part.0);
    if let Some(chat_part) = &chat_part {
        cmd.args(["-map", "1:v", "-t", &length.to_string()])
            .args(["-c:v", "prores_ks", "-profile:v", "4444", "-pix_fmt", "yuva444p10le", "-f", "mov", "-y"])
            .arg(&chat_part.0);
    }
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true).spawn()?;

    // Overlay frame k shows VOD time start + k / FPS, matching clip frame time k / FPS.
    let mut frames = chat.map(|chat| {
        let (tx, rx) = tokio::sync::mpsc::channel::<Arc<Vec<u8>>>(4);
        let start = interval.start;
        let count = (length * f64::from(overlay::FPS)).ceil() as u64 + 1;
        tauri::async_runtime::spawn_blocking(move || {
            let mut overlay = Overlay::new(chat.style, chat.messages, chat.images);
            for k in 0..count {
                let t = start + k as f64 / f64::from(overlay::FPS);
                // Fails once the receiver is gone: the cut finished or was dropped.
                if tx.blocking_send(overlay.frame(t)).is_err() {
                    break;
                }
            }
        });
        rx
    });
    let stdin = child.stdin.take();
    let write_frames = async {
        let (Some(mut stdin), Some(frames)) = (stdin, frames.as_mut()) else { return };
        while let Some(frame) = frames.recv().await {
            // ffmpeg stops reading once it has `-t` worth of video.
            if stdin.write_all(&frame).await.is_err() {
                break;
            }
        }
    };

    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let read_progress = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Some(line) = lines.next_line().await? {
            if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                on_progress((us / 1e6 / length).clamp(0.0, 1.0));
            }
        }
        Ok::<_, std::io::Error>(())
    };
    let read_errors = async {
        let mut text = Vec::new();
        stderr.read_to_end(&mut text).await.map(|_| text)
    };
    let (progress, errors, ()) = tokio::join!(read_progress, read_errors, write_frames);
    progress?;
    let status = child.wait().await?;
    let errors = String::from_utf8_lossy(&errors?).into_owned();
    if !status.success() {
        return Err(Error::Tool(format!("ffmpeg: {}", last_error(&errors))));
    }
    if let Some(line) = errors.lines().find(|l| l.contains("failed too many times")) {
        return Err(Error::SegmentSkipped(strip_level(line)));
    }
    std::fs::rename(&part.0, out)?;
    if let Some(chat_part) = &chat_part {
        std::fs::rename(&chat_part.0, chat_path(out))?;
    }
    on_progress(1.0);
    Ok(())
}

/// Joins `clips` (cut from one video with the same settings, so no re-encoding)
/// into `out`, in order.
pub async fn concat(tools: &Tools, clips: &[PathBuf], out: &Path) -> Result<()> {
    let part = PartFile(out.with_extension("mp4.part"));
    let list = PartFile(out.with_extension("concat.txt"));
    // Single quotes keep everything literal but a quote, which ends and reopens them.
    let text: String = clips
        .iter()
        .map(|c| format!("file '{}'\n", c.to_string_lossy().replace('\'', r"'\''")))
        .collect();
    std::fs::write(&list.0, text)?;
    let output = tokio_command(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "level+error", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list.0)
        .args(["-map", "0", "-c", "copy", "-movflags", "+faststart", "-f", "mp4", "-y"])
        .arg(&part.0)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        return Err(Error::Tool(format!("ffmpeg: {}", last_error(&String::from_utf8_lossy(&output.stderr)))));
    }
    std::fs::rename(&part.0, out)?;
    Ok(())
}

/// Removed on drop unless renamed away first.
struct PartFile(PathBuf);

impl Drop for PartFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn tokio_command(program: &Path) -> tokio::process::Command {
    tokio::process::Command::from(tools::command(program))
}

/// Last non-empty stderr line, which is where both tools put the actual error.
fn tool_error(stderr: &[u8], tool: &str) -> Error {
    let text = String::from_utf8_lossy(stderr);
    let line = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    Error::Tool(format!("{tool}: {}", line.trim().trim_start_matches("ERROR: ")))
}

/// Last error line of ffmpeg stderr written with `-loglevel level+...`; the last
/// line of any level if none is marked as an error.
fn last_error(stderr: &str) -> String {
    let lines = || stderr.lines().rev().filter(|l| !l.trim().is_empty());
    let line = lines()
        .find(|l| l.contains("[error]") || l.contains("[fatal]"))
        .or_else(|| lines().next())
        .unwrap_or("");
    strip_level(line)
}

fn strip_level(line: &str) -> String {
    ["[error] ", "[fatal] ", "[warning] "]
        .iter()
        .fold(line.trim().to_owned(), |l, tag| l.replacen(tag, "", 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn smoke_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wclip-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Frame of `video` at `t` seconds as RGBA.
    fn frame_at(tools: &Tools, video: &Path, t: f64) -> RgbaImage {
        let png = video.with_extension(format!("{t}.png"));
        let ok = tools::command(&tools.ffmpeg)
            .args(["-v", "error", "-y", "-i"])
            .arg(video)
            .args(["-ss", &t.to_string(), "-frames:v", "1"])
            .arg(&png)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        image::open(&png).unwrap().to_rgba8()
    }

    /// Real chat over 10 s of a real VOD; saves a frame to `target/overlay-clip.png`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_chat_overlay() {
        let dir = smoke_dir("overlay");
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");
        let client = reqwest::Client::new();
        let cache = ImageCache::new(dir.join("images"), client.clone());
        let vod = vod_info(&tools, "2905205256", None).await.unwrap();
        let interval = Interval { start: 600.0, end: 610.0, name: String::new() };

        let started = Instant::now();
        let chat = load_chat(&client, &cache, &vod, &interval, &clip_style(&OverlayStyle::default(), &vod, None)).await.unwrap();
        println!("chat: {} messages, {} images ({} animated), {:?}", chat.messages.len(), chat.images.stills.len(), chat.images.animations.len(), started.elapsed());

        let out = dir.join("clip.mp4");
        let started = Instant::now();
        cut(&tools, &vod.playlist, &interval, Some(chat), CutOptions::default(), &out, |_| {}).await.unwrap();
        let took = started.elapsed().as_secs_f64();
        println!("encode: {took:.1} s for 10 s ({:.2}x real time)", 10.0 / took);

        let plain = dir.join("plain.mp4");
        let started = Instant::now();
        cut(&tools, &vod.playlist, &interval, None, CutOptions::default(), &plain, |_| {}).await.unwrap();
        println!("without chat: {:.1} s", started.elapsed().as_secs_f64());

        let frame = frame_at(&tools, &out, 9.0);
        frame.save(concat!(env!("CARGO_MANIFEST_DIR"), "/target/overlay-clip.png")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A message at interval start + 2 s must be absent one overlay frame before and
    /// visible two frames after: `cargo test -- --ignored smoke_overlay_sync`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_overlay_sync() {
        let dir = smoke_dir("sync");
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");
        let vod = vod_info(&tools, "2905205256", None).await.unwrap();
        let interval = Interval { start: 600.0, end: 604.0, name: String::new() };
        let magenta = image::Rgba([255, 0, 255, 255]);
        let message = ChatMessage {
            time: interval.start + 2.0,
            name: "sync".into(),
            color: [255, 255, 255],
            badges: Vec::new(),
            parts: vec![Part::Emote { name: "m".into(), url: "m".into() }],
        };
        let style = OverlayStyle { background: 1.0, ..OverlayStyle::default() }.scaled(vod.height.unwrap());
        let (x, y, style_width, style_height) = (style.x, style.y, style.width, style.height);
        let chat = ChatOverlay {
            style,
            messages: vec![message],
            images: HashMap::from([("m".to_owned(), RgbaImage::from_pixel(300, 100, magenta))]).into(),
        };
        let out = dir.join("clip.mp4");
        cut(&tools, &vod.playlist, &interval, Some(chat), CutOptions::default(), &out, |_| {}).await.unwrap();

        // Only the panel (black background), so magenta in the video itself does not count.
        let magenta_pixels = |img: &RgbaImage| {
            let panel = image::imageops::crop_imm(img, x, y, style_width, style_height).to_image();
            // Low threshold: the message fades in from transparent.
            panel.pixels().filter(|p| p[0] > 40 && p[1] < 40 && p[2] > 40).count()
        };
        let frame = 1.0 / f64::from(overlay::FPS);
        let before = magenta_pixels(&frame_at(&tools, &out, 2.0 - frame));
        // Halfway through the slide: it starts at rest, below the panel bottom.
        let after = magenta_pixels(&frame_at(&tools, &out, 2.0 + overlay::feed::SLIDE / 2.0));
        let settled = magenta_pixels(&frame_at(&tools, &out, 3.0));
        println!("magenta pixels: before {before}, after {after}, settled {settled}");
        assert_eq!(before, 0, "message shown early");
        assert!(after > 1000, "message late");
        assert!(settled > 5000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A local OBS replay with a Chatterino log: the file name gives the start time,
    /// a message 3 s into the video appears 2 s into a clip cut from 1 s.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_local_log() {
        let dir = smoke_dir("local");
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");
        let video = dir.join("Replay 2026-10-19 20-00-06.mkv");
        let ok = tools::command(&tools.ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=gray:s=1280x720:r=30:d=6"])
            .args(["-f", "lavfi", "-i", "sine=d=6", "-c:v", "libx264", "-c:a", "aac", "-shortest"])
            .arg(&video)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let log_path = dir.join("testchan-2026-10-19.log");
        let log_text = "# Start logging at 2026-10-19 19:59:00 MSK\n\
                        [19:59:30] early: before the recording\n\
                        [20:00:03] alice: Hello from the log\n";
        std::fs::write(&log_path, log_text).unwrap();

        let vod = local_info(&tools, &video).await.unwrap();
        assert!(vod.local && (vod.duration - 6.0).abs() < 0.1, "{vod:?}");
        assert_eq!(vod.height, Some(720));
        let started = vod.started.unwrap();
        // Off by the container duration's padding (AAC priming): fine for a chat.
        assert!((started - chat::log::naive_seconds(2026, 10, 19, 20, 0, 0.0)).abs() < 0.1);

        let log = chat::log::open(&log_path).unwrap();
        let all = chat::from_log(&reqwest::Client::new(), &log, started.round()).await;
        assert_eq!(all.iter().map(|m| m.time).collect::<Vec<_>>(), [-30.0, 3.0]);

        let interval = Interval { start: 1.0, end: 5.0, name: String::new() };
        let style = OverlayStyle { lifetime: 10.0, ..OverlayStyle::default() };
        let cache = ImageCache::new(dir.join("images"), reqwest::Client::new());
        let chat = chat_from(&cache, &interval, &clip_style(&style, &vod, None), &all).await;
        assert_eq!(chat.messages.len(), 1, "the early one is out of lookback");
        let (x, y, w, h) = (chat.style.x, chat.style.y, chat.style.width, chat.style.height);
        let out = dir.join("clip.mp4");
        cut(&tools, &vod.playlist, &interval, Some(chat), CutOptions::default(), &out, |_| {}).await.unwrap();

        let bright = |img: &RgbaImage| {
            let panel = image::imageops::crop_imm(img, x, y, w, h).to_image();
            panel.pixels().filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200).count()
        };
        let before = bright(&frame_at(&tools, &out, 1.5));
        let frame = frame_at(&tools, &out, 3.0);
        frame.save(concat!(env!("CARGO_MANIFEST_DIR"), "/target/local-clip.png")).unwrap();
        let after = bright(&frame);
        println!("bright panel pixels: before {before}, after {after}");
        assert_eq!(before, 0);
        assert!(after > 200);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Network smoke test with tools from `PATH`: `cargo test -- --ignored smoke`.
    #[tokio::test]
    #[ignore]
    async fn smoke_cut_30s() {
        let dir = std::env::temp_dir().join(format!("wclip-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");

        let vod = vod_info(&tools, "2905205256", None).await.unwrap();
        assert!(vod.duration > 1000.0);
        let interval = Interval { start: 600.0, end: 630.0, name: String::new() };
        let out = dir.join("clip.mp4");
        cut(&tools, &vod.playlist, &interval, None, CutOptions::default(), &out, |_| {}).await.unwrap();

        let probe = tools::command(&tools.ffprobe)
            .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let duration: f64 = String::from_utf8_lossy(&probe.stdout).trim().parse().unwrap();
        assert!((duration - 30.0).abs() <= 0.1, "duration {duration}");
        assert!(!out.with_extension("mp4.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Dropping `cut` mid-way must kill ffmpeg and leave no partial file.
    #[cfg(unix)]
    #[tokio::test]
    #[ignore]
    async fn smoke_cancel() {
        let dir = std::env::temp_dir().join(format!("wclip-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");
        let vod = vod_info(&tools, "2905205256", None).await.unwrap();
        let interval = Interval { start: 600.0, end: 900.0, name: String::new() };
        let out = dir.join("clip.mp4");
        let part = out.with_extension("mp4.part");

        let run = cut(&tools, &vod.playlist, &interval, None, CutOptions::default(), &out, |_| {});
        let timed_out = tokio::time::timeout(Duration::from_secs(5), run).await.is_err();
        assert!(timed_out, "300 s clip should not finish in 5 s");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!part.exists() && !out.exists());
        let pattern = part.to_string_lossy().into_owned();
        let alive = std::process::Command::new("pgrep").args(["-f", &pattern]).output().unwrap();
        assert!(!alive.status.success(), "ffmpeg still running");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A segment missing from the playlist must fail the clip, not leave a hole.
    /// Offline, needs only ffmpeg in `PATH`: `cargo test -- --ignored skipped_segment`.
    #[tokio::test]
    #[ignore]
    async fn skipped_segment() {
        let dir = smoke_dir("skip");
        let tools = Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), yt_dlp: "yt-dlp".into() };
        let ok = tools::command(&tools.ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=320x180:rate=30", "-t", "12"])
            .args(["-c:v", "libx264", "-g", "30", "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod"])
            .arg(dir.join("v.m3u8"))
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let playlist = dir.join("v.m3u8").to_string_lossy().into_owned();
        let interval = Interval { start: 1.0, end: 10.0, name: String::new() };

        let whole = dir.join("whole.mp4");
        cut(&tools, &playlist, &interval, None, CutOptions::default(), &whole, |_| {}).await.unwrap();

        std::fs::remove_file(dir.join("v2.ts")).unwrap();
        let out = dir.join("holed.mp4");
        let err = cut(&tools, &playlist, &interval, None, CutOptions::default(), &out, |_| {}).await.unwrap_err();
        assert!(matches!(err, Error::SegmentSkipped(_)), "{err}");
        assert_eq!(err.kind(), Kind::Network);
        assert!(!out.exists() && !out.with_extension("mp4.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn probe(tools: &Tools, file: &Path, entries: &str) -> String {
        let out = tools::command(&tools.ffprobe)
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries", entries, "-of", "csv=p=0"])
            .arg(file)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// The chat as its own file, the GPU encoder `Auto` picks and joining clips;
    /// offline, ffmpeg from `PATH`: `cargo test -- --ignored smoke_outputs --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_outputs() {
        let dir = smoke_dir("outputs");
        let tools = Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), yt_dlp: "yt-dlp".into() };
        let source = dir.join("source.mp4");
        let ok = tools::command(&tools.ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "testsrc=size=1280x720:rate=30:duration=6"])
            .args(["-f", "lavfi", "-i", "sine=d=6", "-c:v", "libx264", "-c:a", "aac", "-shortest"])
            .arg(&source)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let gpus = crate::encode::available(&tools, crate::encode::Codec::H264).await;
        let video = crate::encode::resolve(Output::default(), &gpus);
        println!("GPU encoders {gpus:?}, using {:?}", video.encoder);

        let message = ChatMessage {
            time: 1.0,
            name: "alice".into(),
            color: [255, 255, 255],
            badges: Vec::new(),
            parts: vec![Part::Text("hello".into())],
        };
        let style = OverlayStyle::default().scaled(720);
        let chat = ChatOverlay { style: style.clone(), messages: vec![message], images: Images::default() };
        let options = CutOptions { video, chat: ChatOutput::Both, layout: None };
        let playlist = source.to_string_lossy().into_owned();
        let (a, b) = (dir.join("a.mp4"), dir.join("b.mp4"));
        for (out, start) in [(&a, 0.5), (&b, 3.0)] {
            let interval = Interval { start, end: start + 2.0, name: String::new() };
            cut(&tools, &playlist, &interval, Some(chat.clone()), options.clone(), out, |_| {}).await.unwrap();
        }
        let chat_file = chat_path(&a);
        // ProRes 4444 decodes as 12 bit whatever it was encoded from.
        let stream = probe(&tools, &chat_file, "stream=codec_name,width,pix_fmt");
        assert!(stream.starts_with(&format!("prores,{},yuva444", style.width)), "{stream}");
        assert!((probe(&tools, &chat_file, "format=duration").parse::<f64>().unwrap() - 2.0).abs() < 0.1);
        assert!(!chat_file.with_extension("mov.part").exists());

        let merged = dir.join("merged.mp4");
        concat(&tools, &[a, b], &merged).await.unwrap();
        let duration: f64 = probe(&tools, &merged, "format=duration").parse().unwrap();
        assert!((duration - 4.0).abs() < 0.1, "merged {duration}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A vertical clip of a blue frame with a red top-right quadrant, the quadrant
    /// on top and the chat below it; offline, ffmpeg from `PATH`:
    /// `cargo test -- --ignored smoke_vertical --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_vertical() {
        use crate::layout::{self, Area, ChatBox, Piece};
        let dir = smoke_dir("vertical");
        let tools = Tools { ffmpeg: "ffmpeg".into(), ffprobe: "ffprobe".into(), yt_dlp: "yt-dlp".into() };
        let source = dir.join("source.mp4");
        let ok = tools::command(&tools.ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i", "color=c=blue:s=1280x720:r=30:d=4,drawbox=x=640:y=0:w=640:h=360:color=red:t=fill"])
            .args(["-f", "lavfi", "-i", "sine=d=4", "-c:v", "libx264", "-c:a", "aac", "-shortest"])
            .arg(&source)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let cam = Piece { name: "cam".into(), src: Area { x: 0.5, y: 0.0, w: 0.5, h: 0.5 }, x: 0, y: 0, width: 1080 };
        let chat_box = ChatBox { x: 40, y: 1200, width: 600, height: 400, font_size: 30.0 };
        let layout = layout::Layout { pieces: vec![cam], chat: chat_box, ..layout::Layout::default() };
        let vod = Vod {
            id: String::new(),
            title: String::new(),
            channel: String::new(),
            duration: 4.0,
            thumbnail: None,
            playlist: String::new(),
            height: Some(720),
            local: true,
            started: None,
        };
        let style = clip_style(&OverlayStyle { background: 1.0, ..OverlayStyle::default() }, &vod, Some(&layout));
        assert_eq!((style.x, style.y, style.width, style.height, style.font_size), (40, 1200, 600, 400, 30.0));
        let message =
            ChatMessage { time: 0.5, name: "alice".into(), color: [255, 255, 255], badges: Vec::new(), parts: vec![Part::Text("hi".into())] };
        let chat = ChatOverlay { style, messages: vec![message], images: Images::default() };
        let options = CutOptions { layout: Some(layout), ..CutOptions::default() };
        let out = dir.join("vertical.mp4");
        let interval = Interval { start: 1.0, end: 3.0, name: String::new() };
        cut(&tools, &source.to_string_lossy(), &interval, Some(chat), options, &out, |_| {}).await.unwrap();
        assert_eq!(probe(&tools, &out, "stream=width,height").lines().next().unwrap(), "1080,1920");
        let img = frame_at(&tools, &out, 1.0);
        let px = |x, y| img.get_pixel(x, y).0;
        let red = |p: [u8; 4]| p[0] > 200 && p[1] < 60 && p[2] < 60;
        // The quadrant is 640x360, so 1080x608 on the canvas.
        assert!(red(px(540, 300)) && red(px(10, 590)), "{:?} {:?}", px(540, 300), px(10, 590));
        assert!(!red(px(540, 640)), "{:?}", px(540, 640));
        // The panel background is opaque black; the blurred frame around it is bluish.
        let p = px(300, 1500);
        assert!(p[0] < 30 && p[1] < 30 && p[2] < 30, "panel {p:?}");
        let p = px(900, 1800);
        assert!(p[2] > 120, "background {p:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 20 s of a real VOD laid out "camera on top, game below" with chat, on the CPU
    /// and on each GPU encoder found; prints the time and keeps a frame of each:
    /// `cargo test --release -- --ignored smoke_vertical_vod --nocapture`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_vertical_vod() {
        use crate::layout::{Area, ChatBox, Layout, Piece};
        let dir = smoke_dir("vertical-vod");
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");
        let vod = vod_info(&tools, "2905205256", None).await.unwrap();
        let game = Piece { name: "game".into(), src: Area { x: 0.15, y: 0.0, w: 0.7, h: 1.0 }, x: 0, y: 608, width: 1080 };
        let cam = Piece { name: "cam".into(), src: Area { x: 0.74, y: 0.0, w: 0.26, h: 0.26 }, x: 0, y: 0, width: 1080 };
        let chat_box = ChatBox { x: 40, y: 1490, width: 1000, height: 400, font_size: 30.0 };
        let layout = Layout { pieces: vec![game, cam], chat: chat_box, ..Layout::default() };
        let interval = Interval { start: 600.0, end: 620.0, name: String::new() };
        let cache = ImageCache::new(dir.join("images"), reqwest::Client::new());
        let style = clip_style(&OverlayStyle::default(), &vod, Some(&layout));
        let chat = load_chat(&reqwest::Client::new(), &cache, &vod, &interval, &style).await.unwrap();
        let mut encoders = vec![Output::default()];
        for gpu in crate::encode::available(&tools, crate::encode::Codec::H264).await {
            encoders.push(Output { encoder: gpu, ..Output::default() });
        }
        for video in encoders {
            let out = dir.join(format!("{:?}.mp4", video.encoder));
            let options = CutOptions { video, layout: Some(layout.clone()), ..CutOptions::default() };
            let started = Instant::now();
            cut(&tools, &vod.playlist, &interval, Some(chat.clone()), options, &out, |_| {}).await.unwrap();
            println!("{:?}: 20 s in {:.1} s", video.encoder, started.elapsed().as_secs_f64());
            assert_eq!(probe(&tools, &out, "stream=width,height").lines().next().unwrap(), "1080,1920");
            frame_at(&tools, &out, 10.0).save(out.with_extension("png")).unwrap();
        }
        println!("frames in {}", dir.display());
    }

    #[test]
    fn ffmpeg_error_line() {
        let stderr = "[in#0/hls @ 0x5] [warning] Packet corrupt (stream = 0, dts = 1).\n\
                      [out#0/mp4 @ 0x6] [error] Error writing trailer: No space left on device\n\
                      [aac @ 0x7] [warning] Qavg: 1.0\n";
        assert_eq!(last_error(stderr), "[out#0/mp4 @ 0x6] Error writing trailer: No space left on device");
        assert_eq!(last_error("Unknown encoder 'x'\n\n"), "Unknown encoder 'x'");
    }

    #[tokio::test]
    #[ignore]
    async fn smoke_missing_vod() {
        let dir = std::env::temp_dir();
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");
        let err = vod_info(&tools, "1", None).await.unwrap_err();
        assert_eq!(err.kind(), Kind::VodMissing, "{err}");
    }

    #[test]
    fn cookie_file() {
        let cookies = CookieFile::new(" abc123 ").unwrap();
        let text = std::fs::read_to_string(&cookies.0).unwrap();
        assert!(text.ends_with(".twitch.tv\tTRUE\t/\tTRUE\t4102444800\tauth-token\tabc123\n"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&cookies.0).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let path = cookies.0.clone();
        drop(cookies);
        assert!(!path.exists());
        for bad in ["", "abc\tdef", "abc\n#", "OAuth abc"] {
            assert!(matches!(CookieFile::new(bad), Err(Error::BadToken)), "{bad:?}");
        }
    }

    /// Subscriber-only VOD of zerator (checked 2026-10-20; may be deleted later)
    /// without a token and with a wrong one; a public VOD ignores a wrong token.
    #[tokio::test]
    #[ignore]
    async fn smoke_sub_only() {
        let tools = Tools::from_status(&tools::status(&std::env::temp_dir())).expect("tools in PATH");
        let wrong = Some("abcdefghijklmnopqrstuvwxyz0123");
        let err = vod_info(&tools, "2905970151", None).await.unwrap_err();
        assert_eq!(err.kind(), Kind::SubOnly, "{err}");
        let err = vod_info(&tools, "2905970151", wrong).await.unwrap_err();
        assert_eq!(err.kind(), Kind::BadToken, "{err}");
        vod_info(&tools, "2905205256", wrong).await.unwrap();
        let leftover = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&format!("wclip-cookies-{}-", std::process::id())));
        assert!(!leftover);
    }
}
