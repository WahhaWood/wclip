//! Job model: one VOD and the intervals to cut from it (see GLOSSARY.md).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vod {
    pub id: String,
    pub title: String,
    /// Display name of the channel.
    pub channel: String,
    /// Seconds.
    pub duration: f64,
    pub thumbnail: Option<String>,
    /// Direct HLS playlist of the best quality; ffmpeg reads it as input.
    pub playlist: String,
    /// Video height of that playlist, pixels.
    pub height: Option<u32>,
    /// HLS playlist of the audio alone, when Twitch has one: speech recognition
    /// reads ~35 times less from it than from `playlist`.
    #[serde(default)]
    pub audio: Option<String>,
    /// A video file on disk rather than a Twitch VOD: `playlist` is its path, `id`
    /// its name, and the chat comes from `Job::chat_log`.
    #[serde(default)]
    pub local: bool,
    /// Naive local time the file starts at, when its name says (see
    /// `chat::log::started_from_name`).
    #[serde(default)]
    pub started: Option<f64>,
}

impl Vod {
    /// Input to take the sound from.
    pub fn audio_input(&self) -> &str {
        self.audio.as_deref().unwrap_or(&self.playlist)
    }
}

/// Chat log for a local video.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatLogRef {
    pub path: PathBuf,
    /// Chat time at 0:00 of the video.
    pub start: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interval {
    /// Seconds from the start of the VOD.
    pub start: f64,
    pub end: f64,
    /// Clip file name without extension; empty means "make one up".
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub vod: Vod,
    pub intervals: Vec<Interval>,
    #[serde(default)]
    pub chat_log: Option<ChatLogRef>,
    /// Captions edited before cutting, by interval; the missing ones are transcribed.
    #[serde(default)]
    pub captions: Vec<Option<Vec<crate::subtitles::Caption>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "kebab-case")]
pub enum IntervalError {
    #[error("start must be before end")]
    Empty,
    #[error("interval goes past the end of the VOD")]
    PastEnd,
    #[error("start is negative")]
    Negative,
}

impl Interval {
    pub fn check(&self, vod_duration: f64) -> Result<(), IntervalError> {
        if self.start < 0.0 {
            Err(IntervalError::Negative)
        } else if self.start >= self.end {
            Err(IntervalError::Empty)
        } else if self.end > vod_duration {
            Err(IntervalError::PastEnd)
        } else {
            Ok(())
        }
    }
}

impl Job {
    /// Errors by interval index; empty when the job can run.
    pub fn check(&self) -> Vec<(usize, IntervalError)> {
        self.intervals
            .iter()
            .enumerate()
            .filter_map(|(i, iv)| iv.check(self.vod.duration).err().map(|e| (i, e)))
            .collect()
    }
}

/// File name template used when an interval has no name of its own.
pub const DEFAULT_TEMPLATE: &str = "{channel}_{vod}_{start}";

/// Free output path for clip number `n` (from 1): the interval name, or `template`
/// with `{channel}`, `{vod}`, `{start}`, `{end}` (`HH-MM-SS`) and `{n}` filled in;
/// ` (2)`, ` (3)`… is appended if the file exists or is in `taken` (clips still
/// being cut).
pub fn clip_path(dir: &Path, template: &str, vod: &Vod, interval: &Interval, n: usize, taken: &[PathBuf]) -> PathBuf {
    let base = if interval.name.trim().is_empty() {
        fill_template(template, vod, interval, &n.to_string())
    } else {
        interval.name.trim().to_owned()
    };
    free_path(dir, &base, taken)
}

/// Free output path for all clips of a job joined: `template` over the span of
/// `intervals` with `{n}` as "all", and `_merged`.
pub fn merged_path(dir: &Path, template: &str, vod: &Vod, intervals: &[Interval]) -> PathBuf {
    let start = intervals.iter().map(|i| i.start).fold(f64::INFINITY, f64::min);
    let end = intervals.iter().map(|i| i.end).fold(0.0, f64::max);
    let span = Interval { start, end, name: String::new() };
    free_path(dir, &format!("{}_merged", fill_template(template, vod, &span, "all")), &[])
}

fn fill_template(template: &str, vod: &Vod, interval: &Interval, n: &str) -> String {
    let hms = |t: f64| {
        let s = t as u64;
        format!("{:02}-{:02}-{:02}", s / 3600, s / 60 % 60, s % 60)
    };
    let template = if template.trim().is_empty() { DEFAULT_TEMPLATE } else { template };
    template
        .replace("{channel}", &vod.channel)
        .replace("{vod}", &vod.id)
        .replace("{start}", &hms(interval.start))
        .replace("{end}", &hms(interval.end))
        .replace("{n}", n)
        // A local video may have no channel: no `_name_...`.
        .trim_matches(|c| c == '_' || c == '-' || c == ' ')
        .to_owned()
}

/// `<dir>/<base>.mp4` with characters no file system takes replaced, and ` (2)`,
/// ` (3)`… appended while the file exists or is `taken`.
fn free_path(dir: &Path, base: &str, taken: &[PathBuf]) -> PathBuf {
    let base: String = base
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let base = base.trim().trim_end_matches('.');
    (1..)
        .map(|k| match k {
            1 => dir.join(format!("{base}.mp4")),
            k => dir.join(format!("{base} ({k}).mp4")),
        })
        .find(|p| !p.exists() && !taken.contains(p))
        .expect("unbounded")
}

/// VOD id from a bare id or a link like `twitch.tv/videos/<id>` (any host prefix,
/// query or fragment). Also accepts the old `twitch.tv/<channel>/v/<id>` form.
pub fn parse_vod_id(input: &str) -> Option<String> {
    let input = input.trim();
    let is_id = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if is_id(input) {
        return Some(input.to_owned());
    }
    let rest = input.split_once("://").map_or(input, |(_, r)| r);
    let (host, path) = rest.split_once('/')?;
    if host != "twitch.tv" && !host.ends_with(".twitch.tv") {
        return None;
    }
    let path = path.split(['?', '#']).next()?;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    segments
        .windows(2)
        .find(|w| matches!(w[0], "videos" | "v") && is_id(w[1]))
        .map(|w| w[1].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vod_id_forms() {
        let id = Some("2905205256".to_owned());
        for input in [
            "2905205256",
            " 2905205256\n",
            "https://www.twitch.tv/videos/2905205256",
            "https://m.twitch.tv/videos/2905205256?t=1h2m3s",
            "twitch.tv/videos/2905205256/",
            "http://twitch.tv/videos/2905205256#chat",
            "https://www.twitch.tv/somechannel/v/2905205256",
        ] {
            assert_eq!(parse_vod_id(input), id, "{input:?}");
        }
    }

    #[test]
    fn vod_id_rejects() {
        for input in [
            "",
            "abc",
            "https://www.twitch.tv/somechannel",
            "https://www.twitch.tv/somechannel/clip/Abc",
            "https://example.com/videos/2905205256",
            "https://nottwitch.tv/videos/2905205256",
            "https://www.twitch.tv/videos/12ab",
        ] {
            assert_eq!(parse_vod_id(input), None, "{input:?}");
        }
    }

    fn job(intervals: &[(f64, f64)]) -> Job {
        Job {
            vod: Vod {
                id: "1".into(),
                title: String::new(),
                channel: String::new(),
                duration: 100.0,
                thumbnail: None,
                playlist: String::new(),
                height: None,
                audio: None,
                local: false,
                started: None,
            },
            intervals: intervals
                .iter()
                .map(|&(start, end)| Interval { start, end, name: String::new() })
                .collect(),
            chat_log: None,
            captions: Vec::new(),
        }
    }

    #[test]
    fn clip_names() {
        let dir = std::env::temp_dir().join(format!("wclip-test-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut job = job(&[(3725.0, 3800.0)]);
        job.vod.channel = "chan".into();
        let iv = &mut job.intervals[0];
        assert_eq!(clip_path(&dir, "", &job.vod, iv, 1, &[]), dir.join("chan_1_01-02-05.mp4"));
        let local = Vod { channel: String::new(), id: "Replay 1".into(), ..job.vod.clone() };
        assert_eq!(clip_path(&dir, "", &local, iv, 1, &[]), dir.join("Replay 1_01-02-05.mp4"));
        let custom = "{n}. {channel} {start}-{end}";
        assert_eq!(clip_path(&dir, custom, &job.vod, iv, 3, &[]), dir.join("3. chan 01-02-05-01-03-20.mp4"));

        iv.name = " a/b: c? ".into();
        assert_eq!(clip_path(&dir, custom, &job.vod, iv, 1, &[]), dir.join("a_b_ c_.mp4"));
        std::fs::write(dir.join("a_b_ c_.mp4"), b"").unwrap();
        assert_eq!(clip_path(&dir, custom, &job.vod, iv, 1, &[]), dir.join("a_b_ c_ (2).mp4"));
        // A clip still being cut has no file yet.
        assert_eq!(clip_path(&dir, custom, &job.vod, iv, 1, &[dir.join("a_b_ c_ (2).mp4")]), dir.join("a_b_ c_ (3).mp4"));

        let ivs = [Interval { start: 70.0, end: 80.0, name: "x".into() }, Interval { start: 10.0, end: 20.0, name: String::new() }];
        assert_eq!(merged_path(&dir, "", &job.vod, &ivs), dir.join("chan_1_00-00-10_merged.mp4"));
        assert_eq!(merged_path(&dir, custom, &job.vod, &ivs), dir.join("all. chan 00-00-10-00-01-20_merged.mp4"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn interval_checks() {
        let job = job(&[(0.0, 100.0), (10.0, 10.0), (20.0, 5.0), (90.0, 100.5), (-1.0, 5.0)]);
        assert_eq!(
            job.check(),
            vec![
                (1, IntervalError::Empty),
                (2, IntervalError::Empty),
                (3, IntervalError::PastEnd),
                (4, IntervalError::Negative),
            ]
        );
    }
}
