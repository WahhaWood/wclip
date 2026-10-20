//! VOD chat download via GQL `VideoCommentsByOffsetOrCursor`.
//!
//! Paging by cursor fails Twitch's integrity check (checked 2026-10-20), paging by
//! offset does not. So every page is requested by offset: the next request starts at
//! the last comment's offset and duplicates are dropped by id. A page starts several
//! seconds before the requested offset (checked 2026-10-20), so pages overlap and no
//! comment is lost between them. Offsets are whole seconds.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::json;

use super::{Error, GqlError, Result};

const COMMENTS_HASH: &str = "b70a3591ff0f4e0313d126c6a1502d79a1c02baebb288227c582044aa76adf6a";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: String,
    /// Seconds from the start of the VOD.
    pub content_offset_seconds: f64,
    /// `None` for deleted accounts.
    pub commenter: Option<Commenter>,
    pub message: Message,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Commenter {
    pub login: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub fragments: Vec<Fragment>,
    pub user_badges: Vec<Badge>,
    /// `#RRGGBB`, `None` when the user never picked a color.
    pub user_color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fragment {
    pub text: String,
    /// Native Twitch emote; third-party emotes arrive as plain text.
    pub emote: Option<FragmentEmote>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FragmentEmote {
    #[serde(rename = "emoteID")]
    pub emote_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Badge {
    #[serde(rename = "setID")]
    pub set_id: String,
    pub version: String,
}

#[derive(Deserialize)]
struct Response {
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

#[derive(Deserialize)]
struct Data {
    video: Option<Video>,
}

#[derive(Deserialize)]
struct Video {
    comments: Option<Comments>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comments {
    edges: Vec<Edge>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
struct Edge {
    node: Comment,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
}

struct Page {
    comments: Vec<Comment>,
    has_next: bool,
}

async fn fetch_page(client: &reqwest::Client, video_id: &str, offset: u32) -> Result<Page> {
    let body = json!([{
        "operationName": "VideoCommentsByOffsetOrCursor",
        "variables": { "videoID": video_id, "contentOffsetSeconds": offset },
        "extensions": { "persistedQuery": { "version": 1, "sha256Hash": COMMENTS_HASH } },
    }]);
    parse_page(super::post(client, &body).await?)
}

fn parse_page(mut responses: Vec<Response>) -> Result<Page> {
    let response = responses.pop().ok_or_else(|| Error::Gql("empty response".into()))?;
    if let Some(err) = response.errors.into_iter().next() {
        return Err(Error::Gql(err.message));
    }
    let video = response.data.and_then(|d| d.video).ok_or(Error::VideoNotFound)?;
    Ok(match video.comments {
        Some(c) => Page {
            comments: c.edges.into_iter().map(|e| e.node).collect(),
            has_next: c.page_info.has_next_page,
        },
        None => Page { comments: Vec::new(), has_next: false },
    })
}

/// Seconds of VOD one worker pages through; ranges longer than this are split.
const CHUNK: f64 = 120.0;
/// Chunks downloaded at once.
const WORKERS: usize = 32;

/// Comments with `start <= offset < end` (seconds from VOD start), sorted by offset.
/// Long ranges are fetched in `CHUNK`s, `WORKERS` at a time: paging is sequential
/// and slow (30 min of a busy chat takes ~100 s, checked 2026-10-20).
/// `on_progress` gets the share of chunks done, 0..=1.
pub async fn fetch_comments(
    client: &reqwest::Client,
    video_id: &str,
    start: f64,
    end: f64,
    on_progress: impl Fn(f64),
) -> Result<Vec<Comment>> {
    let start = start.max(0.0);
    let chunks: Vec<(f64, f64)> = chunks(start, end).collect();
    let total = chunks.len();
    let mut pending = chunks.into_iter();
    let mut tasks = tokio::task::JoinSet::new();
    let mut spawn_next = |tasks: &mut tokio::task::JoinSet<Result<Vec<Comment>>>| {
        if let Some((a, b)) = pending.next() {
            let (client, id) = (client.clone(), video_id.to_owned());
            tasks.spawn(async move { fetch_range(&client, &id, a, b).await });
        }
    };
    for _ in 0..WORKERS {
        spawn_next(&mut tasks);
    }
    let mut out = Vec::new();
    let mut done = 0;
    while let Some(result) = tasks.join_next().await {
        // A failed chunk fails the whole range; dropping `tasks` aborts the rest.
        out.extend(result.map_err(|e| Error::Gql(e.to_string()))??);
        done += 1;
        on_progress(done as f64 / total as f64);
        spawn_next(&mut tasks);
    }
    out.sort_by(|a, b| a.content_offset_seconds.total_cmp(&b.content_offset_seconds));
    Ok(out)
}

/// `[start, end)` split into consecutive pieces of at most `CHUNK` seconds.
fn chunks(start: f64, end: f64) -> impl Iterator<Item = (f64, f64)> {
    let count = ((end - start) / CHUNK).ceil().max(1.0) as usize;
    (0..count).map(move |i| (start + i as f64 * CHUNK, (start + (i + 1) as f64 * CHUNK).min(end)))
}

/// One range paged sequentially; comments outside `[start, end)` are dropped, so
/// adjacent ranges never share a comment.
async fn fetch_range(client: &reqwest::Client, video_id: &str, start: f64, end: f64) -> Result<Vec<Comment>> {
    let mut offset = start.max(0.0) as u32;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    loop {
        let page = fetch_page(client, video_id, offset).await?;
        let Some(last) = page.comments.last().map(|c| c.content_offset_seconds as u32) else {
            break;
        };
        for c in page.comments {
            let t = c.content_offset_seconds;
            if t >= start && t < end && seen.insert(c.id.clone()) {
                out.push(c);
            }
        }
        if !page.has_next || f64::from(last) >= end {
            break;
        }
        // A page that ends where it started means more comments in one second than
        // a page holds; step forward so we never loop on the same page.
        offset = if last > offset { last } else { offset + 1 };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_page() {
        let responses = serde_json::from_str(include_str!("../../fixtures/comments.json")).unwrap();
        let page = parse_page(responses).unwrap();
        assert!(page.has_next);
        assert_eq!(page.comments.len(), 3);
        let c = &page.comments[0];
        assert_eq!(c.content_offset_seconds, 600.0);
        assert_eq!(c.commenter.as_ref().unwrap().display_name, "Alice");
        assert_eq!(c.message.user_color.as_deref(), Some("#FF7F50"));
        assert_eq!(c.message.user_badges[0].set_id, "subscriber");
        assert_eq!(c.message.fragments[1].emote.as_ref().unwrap().emote_id, "25");
        assert!(page.comments[2].commenter.is_none());
    }

    #[test]
    fn splits_into_chunks() {
        assert_eq!(chunks(10.0, 20.0).collect::<Vec<_>>(), [(10.0, 20.0)]);
        let all: Vec<_> = chunks(0.0, 2.5 * CHUNK).collect();
        assert_eq!(all, [(0.0, CHUNK), (CHUNK, 2.0 * CHUNK), (2.0 * CHUNK, 2.5 * CHUNK)]);
        assert_eq!(chunks(0.0, 2.0 * CHUNK).count(), 2);
    }

    #[test]
    fn missing_video() {
        let responses = serde_json::from_str(r#"[{"data":{"video":null}}]"#).unwrap();
        assert!(matches!(parse_page(responses), Err(Error::VideoNotFound)));
    }
}
