//! Record chat ready for rendering: messages with colors, badge and emote images
//! resolved to URLs.

pub mod cache;
pub mod emotes;
pub mod log;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::twitch::channel::{self, Badges};
use crate::twitch::{self, chat::Comment};
use emotes::Emotes;

#[derive(Debug, Clone, PartialEq)]
pub struct ChatMessage {
    /// Seconds from the start of the video.
    pub time: f64,
    pub name: String,
    pub color: [u8; 3],
    /// Badge image URLs in display order.
    pub badges: Vec<String>,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    Emote { name: String, url: String },
}

/// Which messages the overlay leaves out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChatFilter {
    pub hide_bots: bool,
    /// Names of bots, any case.
    pub bots: Vec<String>,
    /// Messages starting with `!`: commands to a bot.
    pub hide_commands: bool,
}

/// Bots found in most channels.
const BOTS: [&str; 10] = [
    "Nightbot",
    "StreamElements",
    "Streamlabs",
    "Moobot",
    "Fossabot",
    "WizeBot",
    "Sery_Bot",
    "SoundAlerts",
    "Kofistreambot",
    "PokemonCommunityGame",
];

impl Default for ChatFilter {
    fn default() -> Self {
        Self { hide_bots: true, bots: BOTS.map(String::from).to_vec(), hide_commands: false }
    }
}

impl ChatFilter {
    pub fn keeps(&self, m: &ChatMessage) -> bool {
        let bot = self.hide_bots && self.bots.iter().any(|b| b.trim().eq_ignore_ascii_case(&m.name));
        let command = self.hide_commands && matches!(m.parts.first(), Some(Part::Text(t)) if t.trim_start().starts_with('!'));
        !bot && !command
    }
}

/// Colors Twitch assigns to users who never picked one.
const DEFAULT_COLORS: [[u8; 3]; 15] = [
    [0xFF, 0x00, 0x00],
    [0x00, 0x00, 0xFF],
    [0x00, 0x80, 0x00],
    [0xB2, 0x22, 0x22],
    [0xFF, 0x7F, 0x50],
    [0x9A, 0xCD, 0x32],
    [0xFF, 0x45, 0x00],
    [0x2E, 0x8B, 0x57],
    [0xDA, 0xA5, 0x20],
    [0xD2, 0x69, 0x1E],
    [0x5F, 0x9E, 0xA0],
    [0x1E, 0x90, 0xFF],
    [0xFF, 0x69, 0xB4],
    [0x8A, 0x2B, 0xE2],
    [0x00, 0xFF, 0x7F],
];

/// Messages with `start <= time < end`, sorted by time. Pass a `start` earlier than
/// the clip so the overlay does not begin with an empty chat. `on_progress` gets
/// the share of comments downloaded, 0..=1.
pub async fn load(
    client: &reqwest::Client,
    vod_id: &str,
    start: f64,
    end: f64,
    on_progress: impl Fn(f64),
) -> twitch::Result<Vec<ChatMessage>> {
    let (looks, comments) =
        tokio::join!(channel_looks(client, vod_id), twitch::chat::fetch_comments(client, vod_id, start, end, on_progress));
    let ((badges, emotes), comments) = (looks?, comments?);
    Ok(comments.iter().filter_map(|c| build(c, &badges, &emotes)).collect())
}

/// The whole chat of a VOD `duration` long, like `load`. Comments saved in
/// `cache_dir` are used instead of downloading; downloaded ones are saved there.
pub async fn load_whole(
    client: &reqwest::Client,
    vod_id: &str,
    duration: f64,
    cache_dir: &Path,
    on_progress: impl Fn(f64),
) -> twitch::Result<Vec<ChatMessage>> {
    let (dir, id) = (cache_dir.to_owned(), vod_id.to_owned());
    let saved = tokio::task::spawn_blocking(move || cache::read(&dir, &id, duration)).await.ok().flatten();
    let fresh = saved.is_none();
    let comments = async {
        match saved {
            Some(comments) => Ok(comments),
            None => twitch::chat::fetch_comments(client, vod_id, 0.0, duration, on_progress).await,
        }
    };
    let (looks, comments) = tokio::join!(channel_looks(client, vod_id), comments);
    let ((badges, emotes), comments) = (looks?, comments?);
    let messages = comments.iter().filter_map(|c| build(c, &badges, &emotes)).collect();
    if fresh {
        let (dir, id) = (cache_dir.to_owned(), vod_id.to_owned());
        tokio::task::spawn_blocking(move || {
            if let Err(e) = cache::write(&dir, &id, duration, &comments) {
                crate::errlog::write("chat cache", &e.to_string());
            }
        });
    }
    Ok(messages)
}

/// Badges and third-party emotes of the channel that owns the VOD.
async fn channel_looks(client: &reqwest::Client, vod_id: &str) -> twitch::Result<(Badges, Emotes)> {
    let owner = channel::video_owner(client, vod_id).await?;
    let (badges, emotes) = tokio::join!(channel::badges(client, &owner.id), emotes::load(client, &owner.id));
    Ok((badges?, emotes))
}

/// Every message of `log`, timed from 0:00 of a video that starts at chat time
/// `start`. Badges and emotes are best effort: offline the chat stays plain text.
/// Chatterino logs keep Twitch emotes as words; the global ones are recognised.
pub async fn from_log(client: &reqwest::Client, log: &log::ChatLog, start: f64) -> Vec<ChatMessage> {
    let id = match &log.channel_id {
        Some(id) => Some(id.clone()),
        None if !log.info.channel.is_empty() => channel::user_id(client, &log.info.channel).await.ok().flatten(),
        None => None,
    };
    let (badges, third_party, twitch) = match &id {
        Some(id) => {
            let (badges, emotes, twitch) =
                tokio::join!(channel::badges(client, id), emotes::load(client, id), channel::global_emotes(client));
            (badges.unwrap_or_default(), emotes, twitch)
        }
        // Global third-party emotes still apply.
        None => {
            let (emotes, twitch) = tokio::join!(emotes::load(client, ""), channel::global_emotes(client));
            (Badges::new(), emotes, twitch)
        }
    };
    // Third-party emotes win a clash: the chat sees what the extensions show.
    let mut emotes: Emotes = twitch.unwrap_or_default().into_iter().map(|(name, id)| (name, twitch_emote_url(&id))).collect();
    emotes.extend(third_party);
    log.comments
        .iter()
        .filter_map(|c| build(c, &badges, &emotes))
        .map(|m| ChatMessage { time: m.time - start, ..m })
        .collect()
}

/// `None` for comments of deleted accounts: there is no name to show.
pub fn build(comment: &Comment, badges: &Badges, emotes: &Emotes) -> Option<ChatMessage> {
    let user = comment.commenter.as_ref()?;
    let message = &comment.message;
    let mut parts = Vec::new();
    for fragment in &message.fragments {
        match &fragment.emote {
            Some(e) => parts.push(Part::Emote { name: fragment.text.clone(), url: twitch_emote_url(&e.emote_id) }),
            None => push_text(&mut parts, &fragment.text, emotes),
        }
    }
    Some(ChatMessage {
        time: comment.content_offset_seconds,
        name: user.display_name.clone(),
        color: message
            .user_color
            .as_deref()
            .and_then(parse_color)
            .unwrap_or_else(|| default_color(&user.login)),
        badges: message
            .user_badges
            .iter()
            .filter_map(|b| badges.get(&(b.set_id.clone(), b.version.clone())).cloned())
            .collect(),
        parts,
    })
}

/// `default` serves the animated GIF of an animated emote, the PNG of a still one.
fn twitch_emote_url(id: &str) -> String {
    format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/2.0")
}

/// Splits on spaces and turns words found in `emotes` into emote parts; the spaces
/// stay in the text around them.
fn push_text(parts: &mut Vec<Part>, text: &str, emotes: &Emotes) {
    let mut buf = String::new();
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            buf.push(' ');
        }
        match emotes.get(word) {
            Some(url) => {
                if !buf.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut buf)));
                }
                parts.push(Part::Emote { name: word.to_owned(), url: url.clone() });
            }
            None => buf.push_str(word),
        }
    }
    if !buf.is_empty() {
        parts.push(Part::Text(buf));
    }
}

fn parse_color(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let byte = |i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// Stable hash, so a user keeps one color in every clip.
fn default_color(login: &str) -> [u8; 3] {
    let hash = crate::fnv1a(login.as_bytes());
    DEFAULT_COLORS[(hash % DEFAULT_COLORS.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twitch::chat::{Comment, Commenter, Fragment, Message};

    fn comment(login: &str, color: Option<&str>, text: &str) -> Comment {
        Comment {
            id: "1".into(),
            content_offset_seconds: 5.0,
            commenter: Some(Commenter { login: login.into(), display_name: login.to_uppercase() }),
            message: Message {
                fragments: vec![Fragment { text: text.into(), emote: None }],
                user_badges: Vec::new(),
                user_color: color.map(Into::into),
            },
        }
    }

    fn text(s: &str) -> Part {
        Part::Text(s.into())
    }

    fn emote(name: &str, url: &str) -> Part {
        Part::Emote { name: name.into(), url: url.into() }
    }

    #[test]
    fn third_party_emotes_split_text() {
        let emotes = Emotes::from([("OMEGALUL".into(), "u1".into()), ("monkaS".into(), "u2".into())]);
        let mut parts = Vec::new();
        push_text(&mut parts, "OMEGALUL look  monkaS monkaS there OMEGALULx", &emotes);
        assert_eq!(
            parts,
            vec![
                emote("OMEGALUL", "u1"),
                text(" look  "),
                emote("monkaS", "u2"),
                text(" "),
                emote("monkaS", "u2"),
                text(" there OMEGALULx"),
            ]
        );
    }

    #[test]
    fn filter() {
        let m = |name: &str, t: &str| ChatMessage { time: 0.0, name: name.into(), color: [0; 3], badges: Vec::new(), parts: vec![text(t)] };
        let mut f = ChatFilter::default();
        assert!(!f.keeps(&m("nightbot", "hi")), "any case");
        assert!(f.keeps(&m("alice", "!uptime")), "commands kept by default");
        f.hide_commands = true;
        assert!(!f.keeps(&m("alice", "  !uptime")));
        assert!(f.keeps(&m("alice", "wow !")));
        f.hide_bots = false;
        assert!(f.keeps(&m("Nightbot", "hi")));
    }

    #[test]
    fn colors() {
        let none = Badges::new();
        let msg = build(&comment("alice", Some("#1E90FF"), "hi"), &none, &Emotes::new()).unwrap();
        assert_eq!(msg.color, [0x1E, 0x90, 0xFF]);
        assert_eq!(msg.name, "ALICE");

        let a = build(&comment("bob", None, "hi"), &none, &Emotes::new()).unwrap();
        let b = build(&comment("bob", Some("bad"), "hi"), &none, &Emotes::new()).unwrap();
        assert_eq!(a.color, b.color, "same login, same fallback color");
        assert!(DEFAULT_COLORS.contains(&a.color));
    }

    #[test]
    fn builds_fixture_page() {
        #[derive(serde::Deserialize)]
        struct Node {
            node: Comment,
        }
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/comments.json")).unwrap();
        let edges = &raw[0]["data"]["video"]["comments"]["edges"];
        let comments: Vec<Node> = serde_json::from_value(edges.clone()).unwrap();

        let badges = Badges::from([(("subscriber".into(), "12".into()), "sub12".into())]);
        let emotes = Emotes::from([("OMEGALUL".into(), "omg".into())]);
        let msgs: Vec<ChatMessage> =
            comments.iter().filter_map(|c| build(&c.node, &badges, &emotes)).collect();

        assert_eq!(msgs.len(), 2, "deleted account skipped");
        assert_eq!(msgs[0].badges, vec!["sub12".to_owned()], "unknown badge dropped");
        assert_eq!(
            msgs[0].parts,
            vec![
                text("hi "),
                emote("Kappa", "https://static-cdn.jtvnw.net/emoticons/v2/25/default/dark/2.0"),
                text(" look "),
                emote("OMEGALUL", "omg"),
                text(" there"),
            ]
        );
    }

    /// `cargo test -- --ignored smoke_chat`
    #[tokio::test]
    #[ignore]
    async fn smoke_chat() {
        let client = reqwest::Client::new();
        let msgs = load(&client, "2905205256", 600.0, 660.0, |_| {}).await.unwrap();
        assert!(msgs.len() > 50, "{} messages", msgs.len());
        assert!(msgs.windows(2).all(|w| w[0].time <= w[1].time));
        let with_badges = msgs.iter().filter(|m| !m.badges.is_empty()).count();
        let urls: Vec<&str> = msgs
            .iter()
            .flat_map(|m| &m.parts)
            .filter_map(|p| match p {
                Part::Emote { url, .. } => Some(url.as_str()),
                Part::Text(_) => None,
            })
            .collect();
        let third_party = urls.iter().filter(|u| !u.contains("jtvnw.net")).count();
        println!("{} messages, {with_badges} with badges", msgs.len());
        println!("{} emotes, {third_party} third-party", urls.len());
    }

    /// Downloads 10 min of chat, then reads it back from the cache:
    /// `cargo test -- --ignored smoke_whole_cached --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn smoke_whole_cached() {
        let client = reqwest::Client::new();
        let dir = std::env::temp_dir().join(format!("wclip-smoke-chat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let started = std::time::Instant::now();
        let downloaded = load_whole(&client, "2905205256", 600.0, &dir, |_| {}).await.unwrap();
        println!("{} messages downloaded in {:.1} s", downloaded.len(), started.elapsed().as_secs_f64());
        // Saved in the background.
        let file = dir.join("2905205256.json.gz");
        for _ in 0..50 {
            if file.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        println!("cache file {} KB", std::fs::metadata(&file).unwrap().len() / 1024);
        let started = std::time::Instant::now();
        let cached = load_whole(&client, "2905205256", 600.0, &dir, |_| panic!("downloaded again")).await.unwrap();
        println!("read back in {:.1} s", started.elapsed().as_secs_f64());
        assert_eq!(cached, downloaded);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
