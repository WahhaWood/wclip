//! Cutting clips: yt-dlp resolves the VOD's HLS playlist, ffmpeg reads it directly.
//!
//! Input seeking (`-ss` before `-i`) on Twitch HLS is frame-accurate when re-encoding
//! (checked 2026-10-20), so frame 0 of a clip is exactly the interval start and no
//! temporary download is needed.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use crate::job::{Interval, Vod};
use crate::tools::{self, Tools};

const CRF: &str = "18";
const PRESET: &str = "veryfast";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Tool(String),
    #[error("yt-dlp output: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Deserialize)]
struct YtDlpInfo {
    title: String,
    uploader: Option<String>,
    duration: f64,
    url: String,
    thumbnail: Option<String>,
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
    })
}

/// Encodes `interval` of the VOD into `out` (MP4). `on_progress` gets 0..=1.
/// Dropping the future kills ffmpeg and removes the partial file.
pub async fn cut(
    tools: &Tools,
    playlist: &str,
    interval: &Interval,
    out: &Path,
    on_progress: impl Fn(f64),
) -> Result<()> {
    let length = interval.end - interval.start;
    let part = PartFile(out.with_extension("mp4.part"));
    let mut child = tokio_command(&tools.ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-nostats", "-progress", "pipe:1"])
        .args(["-ss", &interval.start.to_string(), "-i", playlist, "-t", &length.to_string()])
        .args(["-map", "0:v:0", "-map", "0:a:0?"])
        .args(["-c:v", "libx264", "-preset", PRESET, "-crf", CRF, "-pix_fmt", "yuv420p"])
        .args(["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", "-f", "mp4", "-y"])
        .arg(&part.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

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
    let (progress, errors) = tokio::join!(read_progress, read_errors);
    progress?;
    let status = child.wait().await?;
    if !status.success() {
        return Err(tool_error(&errors?, "ffmpeg"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
        cut(&tools, &vod.playlist, &interval, &out, |_| {}).await.unwrap();

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

        let run = cut(&tools, &vod.playlist, &interval, &out, |_| {});
        let timed_out = tokio::time::timeout(Duration::from_secs(5), run).await.is_err();
        assert!(timed_out, "300 s clip should not finish in 5 s");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(!part.exists() && !out.exists());
        let pattern = part.to_string_lossy().into_owned();
        let alive = std::process::Command::new("pgrep").args(["-f", &pattern]).output().unwrap();
        assert!(!alive.status.success(), "ffmpeg still running");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn smoke_missing_vod() {
        let dir = std::env::temp_dir();
        let tools = Tools::from_status(&tools::status(&dir)).expect("tools in PATH");
        let err = vod_info(&tools, "1").await.unwrap_err().to_string();
        assert!(err.contains("does not exist"), "{err}");
    }
}
