//! Chat from a file instead of a VOD, for videos Twitch does not serve: Chatterino
//! logs (`[HH:MM:SS] login: text`) and TwitchDownloader JSON.
//!
//! Comments come out as GQL `Comment`s so `chat::build` renders them the same way.
//! Chatterino times are wall-clock: seconds of naive local time since 1970 (see
//! `naive_seconds`); TwitchDownloader times are seconds from the start of the VOD.

use std::path::Path;

use serde::Serialize;

use crate::twitch::chat::{Badge, Comment, Commenter, Fragment, FragmentEmote, Message};

#[derive(Debug, Clone)]
pub struct ChatLog {
    pub info: ChatLogInfo,
    /// Channel id, when the file names it (TwitchDownloader).
    pub channel_id: Option<String>,
    /// Sorted by time.
    pub comments: Vec<Comment>,
}

/// What the UI shows about a chat log.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatLogInfo {
    /// Channel login, empty if the file does not say.
    pub channel: String,
    /// Times are naive local wall-clock seconds, not VOD offsets.
    pub clock: bool,
    /// Chat time likely matching 0:00 of the video: the start of the downloaded
    /// part of the VOD, or the first message of a clock log.
    pub start: f64,
    pub count: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("TwitchDownloader JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no chat messages with a time in the file")]
    Empty,
}

/// A Chatterino log or TwitchDownloader JSON, plain or gzipped (`.json.gz`).
pub fn open(path: &Path) -> Result<ChatLog, Error> {
    let bytes = std::fs::read(path)?;
    let text = if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut flate2::read::GzDecoder::new(bytes.as_slice()), &mut text)?;
        text
    } else {
        String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let log = if text.trim_start().starts_with('{') { twitch_downloader(&text)? } else { chatterino(&text, stem) };
    if log.comments.is_empty() {
        return Err(Error::Empty);
    }
    Ok(log)
}

/// Days from 1970-01-01 to the given civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Naive local date and time as seconds since 1970, without any time zone: both a
/// Chatterino log and an OBS file name are in the local time of the same machine.
pub fn naive_seconds(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: f64) -> f64 {
    (days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60) as f64 + s
}

/// `YYYY-MM-DD` at the start of `s`.
fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.get(..10)?.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// Seconds of the day from `H:MM:SS` with an optional fraction.
fn parse_clock(s: &str) -> Option<f64> {
    let mut it = s.split(':');
    let (h, m, sec) = (it.next()?, it.next()?, it.next()?);
    if it.next().is_some() {
        return None;
    }
    let (h, m, sec): (u32, u32, f64) = (h.parse().ok()?, m.parse().ok()?, sec.parse().ok()?);
    (h < 24 && m < 60 && (0.0..61.0).contains(&sec)).then(|| f64::from(h * 3600 + m * 60) + sec)
}

fn is_login(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Chatterino writes `# Start logging at YYYY-MM-DD HH:MM:SS TZ` on every open,
/// then `[HH:MM:SS] login: text` (`display login: text` for non-Latin display
/// names). Lines without a login are notices (subs, raids) and are skipped. The file
/// is named `<channel>-<date or stream id>.log`.
fn chatterino(text: &str, stem: &str) -> ChatLog {
    let (channel, rest) = stem.split_once('-').unwrap_or((stem, ""));
    let mut day = parse_date(rest).unwrap_or(0);
    let mut last = 0.0;
    let mut comments = Vec::new();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# Start logging at ").or_else(|| line.strip_prefix("# Stop logging at ")) {
            if let Some(d) = parse_date(header) {
                day = d;
                last = header.get(11..19).and_then(parse_clock).unwrap_or(0.0);
            }
            continue;
        }
        let Some((stamp, rest)) = line.strip_prefix('[').and_then(|l| l.split_once("] ")) else { continue };
        // The stamp format is a setting; `hh:mm:ss` by default, a date may lead.
        let (date, clock) = stamp.rsplit_once(' ').map_or((None, stamp), |(d, c)| (Some(d), c));
        let Some(secs) = parse_clock(clock) else { continue };
        match date.and_then(parse_date) {
            Some(d) => day = d,
            // Past midnight without a new header.
            None if secs + 43_200.0 < last => day += 1,
            None => {}
        }
        last = secs;
        let Some((who, body)) = rest.split_once(": ") else { continue };
        let (display, login) = who.rsplit_once(' ').unwrap_or((who, who));
        if !is_login(login) || display.contains(' ') {
            continue;
        }
        comments.push(Comment {
            id: comments.len().to_string(),
            content_offset_seconds: (day * 86_400) as f64 + secs,
            commenter: Some(Commenter { login: login.to_lowercase(), display_name: display.to_owned() }),
            message: Message {
                fragments: vec![Fragment { text: body.to_owned(), emote: None }],
                user_badges: Vec::new(),
                user_color: None,
            },
        });
    }
    comments.sort_by(|a, b| a.content_offset_seconds.total_cmp(&b.content_offset_seconds));
    let start = comments.first().map_or(0.0, |c| c.content_offset_seconds.floor());
    ChatLog {
        info: ChatLogInfo { channel: channel.to_lowercase(), clock: true, start, count: comments.len() },
        channel_id: None,
        comments,
    }
}

/// `TwitchDownloaderCore/TwitchObjects/ChatRoot.cs`; only the fields WClip uses.
mod td {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct Root {
        pub streamer: Option<Streamer>,
        pub video: Option<Video>,
        pub comments: Vec<Comment>,
    }

    #[derive(Deserialize)]
    pub struct Streamer {
        pub login: Option<String>,
        pub name: Option<String>,
        #[serde(default)]
        pub id: serde_json::Value,
    }

    #[derive(Deserialize)]
    pub struct Video {
        /// Start of the downloaded part, seconds from the VOD start.
        #[serde(default)]
        pub start: f64,
    }

    #[derive(Deserialize)]
    pub struct Comment {
        #[serde(rename = "_id", default)]
        pub id: String,
        pub content_offset_seconds: f64,
        pub commenter: Option<Commenter>,
        pub message: Message,
    }

    #[derive(Deserialize)]
    pub struct Commenter {
        pub display_name: String,
        pub name: String,
    }

    #[derive(Deserialize)]
    pub struct Message {
        pub body: String,
        #[serde(default)]
        pub fragments: Option<Vec<Fragment>>,
        #[serde(default)]
        pub user_badges: Option<Vec<Badge>>,
        pub user_color: Option<String>,
    }

    #[derive(Deserialize)]
    pub struct Fragment {
        pub text: String,
        pub emoticon: Option<Emoticon>,
    }

    #[derive(Deserialize)]
    pub struct Emoticon {
        pub emoticon_id: String,
    }

    #[derive(Deserialize)]
    pub struct Badge {
        #[serde(rename = "_id")]
        pub id: String,
        pub version: String,
    }
}

fn twitch_downloader(text: &str) -> Result<ChatLog, Error> {
    let root: td::Root = serde_json::from_str(text)?;
    let comments: Vec<Comment> = root
        .comments
        .into_iter()
        .map(|c| {
            let fragments = match c.message.fragments {
                Some(f) if !f.is_empty() => f
                    .into_iter()
                    .map(|f| Fragment {
                        text: f.text,
                        emote: f.emoticon.map(|e| FragmentEmote { emote_id: e.emoticon_id }),
                    })
                    .collect(),
                _ => vec![Fragment { text: c.message.body, emote: None }],
            };
            Comment {
                id: c.id,
                content_offset_seconds: c.content_offset_seconds,
                commenter: c.commenter.map(|u| Commenter { login: u.name, display_name: u.display_name }),
                message: Message {
                    fragments,
                    user_badges: (c.message.user_badges.unwrap_or_default().into_iter())
                        .map(|b| Badge { set_id: b.id, version: b.version })
                        .collect(),
                    user_color: c.message.user_color,
                },
            }
        })
        .collect();
    let streamer = root.streamer;
    let channel = streamer.as_ref().and_then(|s| s.login.clone().or_else(|| s.name.clone())).unwrap_or_default();
    // A number in new files, a string in old ones.
    let channel_id = streamer.and_then(|s| match s.id {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.is_empty() => Some(s),
        _ => None,
    });
    let start = root.video.map_or(0.0, |v| v.start);
    Ok(ChatLog {
        info: ChatLogInfo { channel: channel.to_lowercase(), clock: false, start, count: comments.len() },
        channel_id,
        comments,
    })
}

/// When a video file name carries its date and time (OBS `2026-10-19 20-00-03.mkv`,
/// ShadowPlay `Game 2026.10.19 - 20.00.03.12.mp4`): the naive local time the video
/// starts at. Replay-buffer files are named when saved, so `duration` is taken off.
pub fn started_from_name(stem: &str, duration: f64) -> Option<f64> {
    let runs: Vec<&str> = stem.split(|c: char| !c.is_ascii_digit()).filter(|r| !r.is_empty()).collect();
    let at = runs.windows(6).find_map(|w| {
        let lens: Vec<usize> = w.iter().map(|r| r.len()).collect();
        if lens != [4, 2, 2, 2, 2, 2] {
            return None;
        }
        let n: Vec<i64> = w.iter().map(|r| r.parse().unwrap_or(-1)).collect();
        let valid = (1..=12).contains(&n[1]) && (1..=31).contains(&n[2]) && n[3] < 24 && n[4] < 60 && n[5] < 60;
        valid.then(|| naive_seconds(n[0], n[1], n[2], n[3], n[4], n[5] as f64))
    })?;
    let lower = stem.to_lowercase();
    Some(if lower.contains("replay") || lower.contains("dvr") { at - duration } else { at })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(c: &Comment) -> &str {
        &c.message.fragments[0].text
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 10, 20), 20_746);
    }

    #[test]
    fn chatterino_log() {
        let text = "\
# Start logging at 2026-10-19 23:59:50 MSK
[23:59:55] alice: hi: there
[23:59:58] Some notice without a login
[23:59:59] 小明 xiaoming: 你好
[00:00:02] bob: after midnight
[00:00:03] two words name: skipped
# Stop logging at 2026-10-20 00:10:00 MSK
# Start logging at 2026-10-21 12:00:00 MSK
[2026-10-21 12:00:01] carol: dated stamp
";
        let log = chatterino(text, "somechannel-2026-10-19");
        assert_eq!(log.info.channel, "somechannel");
        assert!(log.info.clock);
        let day = 20_745.0 * 86_400.0;
        let c = &log.comments;
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].content_offset_seconds, day + 86_395.0);
        assert_eq!(text_of(&c[0]), "hi: there");
        assert_eq!(c[1].commenter.as_ref().unwrap().display_name, "小明");
        assert_eq!(c[1].commenter.as_ref().unwrap().login, "xiaoming");
        assert_eq!(c[2].content_offset_seconds, day + 86_400.0 + 2.0, "next day");
        assert_eq!(c[3].content_offset_seconds, day + 2.0 * 86_400.0 + 43_201.0);
        assert_eq!(log.info.start, day + 86_395.0);
    }

    #[test]
    fn chatterino_stream_log_takes_date_from_header() {
        let text = "# Start logging at 2026-10-19 20:00:00 MSK\n[20:00:05] alice: hi\n";
        let log = chatterino(text, "chan-123456789");
        assert_eq!(log.comments[0].content_offset_seconds, naive_seconds(2026, 10, 19, 20, 0, 5.0));
    }

    #[test]
    fn twitch_downloader_json() {
        let text = r##"{
            "FileInfo": {"Version": {"Major": 1}},
            "streamer": {"name": "Streamer", "login": "streamer", "id": 12345},
            "video": {"title": "t", "id": "1", "start": 600.0, "end": 700.0},
            "comments": [
                {"_id": "a", "content_offset_seconds": 601.5,
                 "commenter": {"display_name": "Alice", "name": "alice"},
                 "message": {"body": "hi Kappa", "fragments": [{"text": "hi ", "emoticon": null},
                    {"text": "Kappa", "emoticon": {"emoticon_id": "25"}}],
                    "user_badges": [{"_id": "subscriber", "version": "12"}], "user_color": "#FF0000"}},
                {"_id": "b", "content_offset_seconds": 602,
                 "commenter": {"display_name": "Bob", "name": "bob"},
                 "message": {"body": "plain", "fragments": null, "user_badges": null, "user_color": null}}
            ]
        }"##;
        let log = twitch_downloader(text).unwrap();
        assert_eq!(log.info, ChatLogInfo { channel: "streamer".into(), clock: false, start: 600.0, count: 2 });
        assert_eq!(log.channel_id.as_deref(), Some("12345"));
        let a = &log.comments[0];
        assert_eq!(a.message.fragments[1].emote.as_ref().unwrap().emote_id, "25");
        assert_eq!(a.message.user_badges[0].set_id, "subscriber");
        assert_eq!(text_of(&log.comments[1]), "plain", "body when there are no fragments");

        let path = std::env::temp_dir().join(format!("wclip-test-{}.json.gz", std::process::id()));
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, text.as_bytes()).unwrap();
        std::fs::write(&path, gz.finish().unwrap()).unwrap();
        assert_eq!(open(&path).unwrap().info, log.info, "gzipped");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn start_from_file_name() {
        let obs = naive_seconds(2026, 10, 19, 20, 0, 3.0);
        assert_eq!(started_from_name("2026-10-19 20-00-03", 30.0), Some(obs));
        assert_eq!(started_from_name("Replay 2026-10-19 20-00-03", 30.0), Some(obs - 30.0));
        assert_eq!(started_from_name("Game 2026.10.19 - 20.00.03.12", 30.0), Some(obs));
        assert_eq!(started_from_name("clip 12", 30.0), None);
        assert_eq!(started_from_name("2026-13-19 20-00-03", 30.0), None);
    }
}
