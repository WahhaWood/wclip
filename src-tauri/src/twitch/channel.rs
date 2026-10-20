//! Channel data for rendering chat: the VOD owner and chat badge images.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::json;

use super::{Error, Result, query};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    /// Numeric user id; third-party emote services key channels by it.
    pub id: String,
    pub login: String,
    pub display_name: String,
}

pub async fn video_owner(client: &reqwest::Client, video_id: &str) -> Result<Owner> {
    #[derive(Deserialize)]
    struct Data {
        video: Option<Video>,
    }
    #[derive(Deserialize)]
    struct Video {
        owner: Option<Owner>,
    }
    let data: Data = query(
        client,
        "query($id:ID!){video(id:$id){owner{id login displayName}}}",
        json!({ "id": video_id }),
    )
    .await?;
    data.video.and_then(|v| v.owner).ok_or(Error::VideoNotFound)
}

/// Numeric id of the channel `login`; `None` if there is no such user.
pub async fn user_id(client: &reqwest::Client, login: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Data {
        user: Option<User>,
    }
    #[derive(Deserialize)]
    struct User {
        id: String,
    }
    let data: Data = query(client, "query($login:String!){user(login:$login){id}}", json!({ "login": login })).await?;
    Ok(data.user.map(|u| u.id))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BadgeImage {
    #[serde(rename = "setID")]
    set_id: String,
    version: String,
    #[serde(rename = "imageURL")]
    image_url: String,
}

/// Image URL by `(set id, version)`; channel badges (subscriber, bits) override global.
pub type Badges = HashMap<(String, String), String>;

pub async fn badges(client: &reqwest::Client, channel_id: &str) -> Result<Badges> {
    #[derive(Deserialize)]
    struct Global {
        badges: Vec<BadgeImage>,
    }
    #[derive(Deserialize)]
    struct Channel {
        user: Option<User>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct User {
        broadcast_badges: Vec<BadgeImage>,
    }
    let global: Global =
        query(client, "query{badges{setID version imageURL(size:DOUBLE)}}", json!({})).await?;
    let channel: Channel = query(
        client,
        "query($id:ID!){user(id:$id){broadcastBadges{setID version imageURL(size:DOUBLE)}}}",
        json!({ "id": channel_id }),
    )
    .await?;
    let channel = channel.user.map(|u| u.broadcast_badges).unwrap_or_default();
    Ok(merge_badges(global.badges, channel))
}

fn merge_badges(global: Vec<BadgeImage>, channel: Vec<BadgeImage>) -> Badges {
    global
        .into_iter()
        .chain(channel)
        .map(|b| ((b.set_id, b.version), b.image_url))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn badge(set: &str, version: &str, url: &str) -> BadgeImage {
        BadgeImage { set_id: set.into(), version: version.into(), image_url: url.into() }
    }

    #[test]
    fn channel_badges_override_global() {
        let merged = merge_badges(
            vec![badge("subscriber", "0", "global-sub"), badge("premium", "1", "prime")],
            vec![badge("subscriber", "0", "chan-sub"), badge("subscriber", "12", "chan-sub-12")],
        );
        let get = |s: &str, v: &str| merged.get(&(s.into(), v.into())).map(String::as_str);
        assert_eq!(get("subscriber", "0"), Some("chan-sub"));
        assert_eq!(get("subscriber", "12"), Some("chan-sub-12"));
        assert_eq!(get("premium", "1"), Some("prime"));
    }
}
