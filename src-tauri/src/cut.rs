//! Cutting clips: yt-dlp resolves the VOD's HLS playlist, ffmpeg reads it directly.
//!
//! Input seeking (`-ss` before `-i`) on Twitch HLS is frame-accurate when re-encoding
//! (checked 2026-10-20), so frame 0 of a clip is exactly the interval start and no
//! temporary download is needed.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use image::RgbaImage;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::chat::{self, ChatMessage, Part};
use crate::failure::{self, Kind};
use crate::images::ImageCache;
use crate::job::{Interval, Vod};
use crate::overlay::{self, Overlay, OverlayStyle};
use crate::tools::{self, Tools};

const CRF: &str = "18";
const PRESET: &str = "veryfast";

/// HLS input options: give up on a stalled read after 20 s, retry a failed
/// segment 3 times before the demuxer skips it.
const HLS_INPUT: [&str; 4] = ["-rw_timeout", "20000000", "-seg_max_retry", "3"];

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
}

impl Error {
    pub fn kind(&self) -> Kind {
        match self {
            Error::Io(e) => failure::io_kind(e),
            Error::Tool(text) => failure::classify(text),
            Error::Json(_) => Kind::Other,
            Error::SegmentSkipped(_) => Kind::Network,
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

pub async fn vod_info(tools: &Tools, id: &str) -> Result<Vod> {
    let out = tokio_command(&tools.yt_dlp)
        .args(["-J", "-f", "b", "--no-warnings", "--no-playlist"])
        .arg(format!("https://www.twitch.tv/videos/{id}"))
        .output()
        .await?;
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
    })
}

/// Chat to burn into a clip.
#[derive(Clone)]
pub struct ChatOverlay {
    /// Already scaled to the video height.
    pub style: OverlayStyle,
    pub messages: Vec<ChatMessage>,
    pub images: HashMap<String, RgbaImage>,
}

/// Video frame at VOD time `t`, for the overlay preview.
pub async fn grab_frame(tools: &Tools, playlist: &str, t: f64) -> Result<RgbaImage> {
    let out = tokio_command(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .args(HLS_INPUT)
        .args(["-ss", &t.to_string(), "-i", playlist])
        .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "pipe:1"])
        .output()
        .await?;
    if !out.status.success() {
        return Err(tool_error(&out.stderr, "ffmpeg"));
    }
    image::load_from_memory_with_format(&out.stdout, image::ImageFormat::Png)
        .map(|img| img.to_rgba8())
        .map_err(|e| Error::Tool(format!("ffmpeg frame: {e}")))
}

/// Loads the chat shown during `interval` (with lookback, so the panel starts
/// filled) and every badge and emote image in it. Images that fail to load are
/// left out; the renderer falls back to text for them.
pub async fn load_chat(
    client: &reqwest::Client,
    cache: &ImageCache,
    vod: &Vod,
    interval: &Interval,
    style: &OverlayStyle,
) -> crate::twitch::Result<ChatOverlay> {
    let style = style.scaled(vod.height.unwrap_or(1080));
    let start = (interval.start - style.lookback()).max(0.0);
    let messages = chat::load(client, &vod.id, start, interval.end).await?;
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
    Ok(ChatOverlay { style, messages, images })
}

/// Encodes `interval` of the VOD into `out` (MP4), with the chat on top if given.
/// `on_progress` gets 0..=1. Dropping the future kills ffmpeg and removes the
/// partial file.
pub async fn cut(
    tools: &Tools,
    playlist: &str,
    interval: &Interval,
    chat: Option<ChatOverlay>,
    out: &Path,
    on_progress: impl Fn(f64),
) -> Result<()> {
    let length = interval.end - interval.start;
    let part = PartFile(out.with_extension("mp4.part"));
    let mut cmd = tokio_command(&tools.ffmpeg);
    // Warnings too: that is where the demuxer reports a skipped segment.
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "level+warning", "-nostats", "-progress", "pipe:1"])
        .args(HLS_INPUT)
        .args(["-ss", &interval.start.to_string(), "-i", playlist]);
    match &chat {
        Some(chat) => {
            let (w, h) = (chat.style.width, chat.style.height);
            let (x, y) = chat.style.position();
            cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}")])
                .args(["-framerate", &overlay::FPS.to_string(), "-i", "pipe:0"])
                .args(["-filter_complex", &format!("[0:v][1:v]overlay=x={x}:y={y}:eof_action=repeat[v]")])
                .args(["-map", "[v]"])
                .stdin(Stdio::piped());
        }
        None => {
            cmd.args(["-map", "0:v:0"]).stdin(Stdio::null());
        }
    }
    let mut child = cmd
        .args(["-map", "0:a:0?", "-t", &length.to_string()])
        .args(["-c:v", "libx264", "-preset", PRESET, "-crf", CRF, "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", "-f", "mp4", "-y"])
        .arg(&part.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

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
    on_progress(1.0);
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
        let vod = vod_info(&tools, "2905205256").await.unwrap();
        let interval = Interval { start: 600.0, end: 610.0, name: String::new() };

        let started = Instant::now();
        let chat = load_chat(&client, &cache, &vod, &interval, &OverlayStyle::default()).await.unwrap();
        println!("chat: {} messages, {} images, {:?}", chat.messages.len(), chat.images.len(), started.elapsed());

        let out = dir.join("clip.mp4");
        let started = Instant::now();
        cut(&tools, &vod.playlist, &interval, Some(chat), &out, |_| {}).await.unwrap();
        let took = started.elapsed().as_secs_f64();
        println!("encode: {took:.1} s for 10 s ({:.2}x real time)", 10.0 / took);

        let plain = dir.join("plain.mp4");
        let started = Instant::now();
        cut(&tools, &vod.playlist, &interval, None, &plain, |_| {}).await.unwrap();
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
        let vod = vod_info(&tools, "2905205256").await.unwrap();
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
        let (style_margin, style_width, style_height) = (style.margin, style.width, style.height);
        let chat = ChatOverlay {
            style,
            messages: vec![message],
            images: HashMap::from([("m".to_owned(), RgbaImage::from_pixel(300, 100, magenta))]),
        };
        let out = dir.join("clip.mp4");
        cut(&tools, &vod.playlist, &interval, Some(chat), &out, |_| {}).await.unwrap();

        // Only the panel (black background), so magenta in the video itself does not count.
        let (x, y) = (style_margin, vod.height.unwrap() - style_height - style_margin);
        let magenta_pixels = |img: &RgbaImage| {
            let panel = image::imageops::crop_imm(img, x, y, style_width, style_height).to_image();
            // Low threshold: the message fades in from transparent.
            panel.pixels().filter(|p| p[0] > 40 && p[1] < 40 && p[2] > 40).count()
        };
        let frame = 1.0 / f64::from(overlay::FPS);
        let before = magenta_pixels(&frame_at(&tools, &out, 2.0 - frame));
        let after = magenta_pixels(&frame_at(&tools, &out, 2.0 + 2.0 * frame));
        let settled = magenta_pixels(&frame_at(&tools, &out, 3.0));
        println!("magenta pixels: before {before}, after {after}, settled {settled}");
        assert_eq!(before, 0, "message shown early");
        assert!(after > 1000, "message late");
        assert!(settled > 5000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Network smoke test with tools from `PATH`: `cargo test -- --ignored smoke`.
    #[tokio::test]
    #[ignore]
    async fn smoke_cut_30s() {
        let dir = std::env::temp_dir().join(format!("wclip-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");

        let vod = vod_info(&tools, "2905205256").await.unwrap();
        assert!(vod.duration > 1000.0);
        let interval = Interval { start: 600.0, end: 630.0, name: String::new() };
        let out = dir.join("clip.mp4");
        cut(&tools, &vod.playlist, &interval, None, &out, |_| {}).await.unwrap();

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
        let vod = vod_info(&tools, "2905205256").await.unwrap();
        let interval = Interval { start: 600.0, end: 900.0, name: String::new() };
        let out = dir.join("clip.mp4");
        let part = out.with_extension("mp4.part");

        let run = cut(&tools, &vod.playlist, &interval, None, &out, |_| {});
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
        cut(&tools, &playlist, &interval, None, &whole, |_| {}).await.unwrap();

        std::fs::remove_file(dir.join("v2.ts")).unwrap();
        let out = dir.join("holed.mp4");
        let err = cut(&tools, &playlist, &interval, None, &out, |_| {}).await.unwrap_err();
        assert!(matches!(err, Error::SegmentSkipped(_)), "{err}");
        assert_eq!(err.kind(), Kind::Network);
        assert!(!out.exists() && !out.with_extension("mp4.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
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
        let err = vod_info(&tools, "1").await.unwrap_err();
        assert_eq!(err.kind(), Kind::VodMissing, "{err}");
    }
}
