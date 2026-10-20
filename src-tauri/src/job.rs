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

/// Free output path for the clip: the interval name, or `<channel>_<vod>_<HH-MM-SS>`,
/// with ` (2)`, ` (3)`… appended if the file exists.
pub fn clip_path(dir: &Path, vod: &Vod, interval: &Interval) -> PathBuf {
    let base = if interval.name.trim().is_empty() {
        let s = interval.start as u64;
        format!("{}_{}_{:02}-{:02}-{:02}", vod.channel, vod.id, s / 3600, s / 60 % 60, s % 60)
    } else {
        interval.name.trim().to_owned()
    };
    let base: String = base
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let base = base.trim_end_matches(['.', ' ']);
    (1..)
        .map(|n| match n {
            1 => dir.join(format!("{base}.mp4")),
            n => dir.join(format!("{base} ({n}).mp4")),
        })
        .find(|p| !p.exists())
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
            },
            intervals: intervals
                .iter()
                .map(|&(start, end)| Interval { start, end, name: String::new() })
                .collect(),
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
        assert_eq!(clip_path(&dir, &job.vod, iv), dir.join("chan_1_01-02-05.mp4"));

        iv.name = " a/b: c? ".into();
        assert_eq!(clip_path(&dir, &job.vod, iv), dir.join("a_b_ c_.mp4"));
        std::fs::write(dir.join("a_b_ c_.mp4"), b"").unwrap();
        assert_eq!(clip_path(&dir, &job.vod, iv), dir.join("a_b_ c_ (2).mp4"));
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
