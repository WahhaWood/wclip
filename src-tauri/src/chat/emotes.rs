//! Third-party emotes: 7TV, BTTV and FFZ (through BTTV's FFZ cache). They arrive in
//! chat as plain words, so the catalog maps a word to an image URL.

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::DeserializeOwned;

/// Image URL by emote name.
pub type Emotes = HashMap<String, String>;

const SEVEN_TV: &str = "https://7tv.io/v3";
const BTTV: &str = "https://api.betterttv.net/3/cached";

/// Global and channel emotes of every provider. Later sources override earlier
/// ones: channel over global, 7TV over BTTV over FFZ. A provider that fails or has
/// no such channel is skipped.
pub async fn load(client: &reqwest::Client, channel_id: &str) -> Emotes {
    let mut out = Emotes::new();
    if let Some(list) = get::<Vec<Ffz>>(client, &format!("{BTTV}/frankerfacez/emotes/global")).await {
        out.extend(ffz(list));
    }
    if let Some(list) = get::<Vec<Bttv>>(client, &format!("{BTTV}/emotes/global")).await {
        out.extend(bttv(list));
    }
    if let Some(set) = get::<SevenTvSet>(client, &format!("{SEVEN_TV}/emote-sets/global")).await {
        out.extend(seven_tv(set));
    }
    let url = format!("{BTTV}/frankerfacez/users/twitch/{channel_id}");
    if let Some(list) = get::<Vec<Ffz>>(client, &url).await {
        out.extend(ffz(list));
    }
    if let Some(user) = get::<BttvUser>(client, &format!("{BTTV}/users/twitch/{channel_id}")).await {
        out.extend(bttv(user.shared_emotes.into_iter().chain(user.channel_emotes).collect()));
    }
    let url = format!("{SEVEN_TV}/users/twitch/{channel_id}");
    if let Some(set) = get::<SevenTvUser>(client, &url).await.and_then(|u| u.emote_set) {
        out.extend(seven_tv(set));
    }
    out
}

async fn get<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Option<T> {
    let resp = client.get(url).send().await.ok()?.error_for_status().ok()?;
    resp.json().await.ok()
}

#[derive(Deserialize)]
struct SevenTvUser {
    emote_set: Option<SevenTvSet>,
}

#[derive(Deserialize)]
struct SevenTvSet {
    #[serde(default)]
    emotes: Vec<SevenTvEmote>,
}

#[derive(Deserialize)]
struct SevenTvEmote {
    name: String,
    data: SevenTvData,
}

#[derive(Deserialize)]
struct SevenTvData {
    host: SevenTvHost,
}

#[derive(Deserialize)]
struct SevenTvHost {
    /// Protocol-relative: `//cdn.7tv.app/emote/<id>`.
    url: String,
    files: Vec<SevenTvFile>,
}

#[derive(Deserialize)]
struct SevenTvFile {
    /// Animated for an animated emote; `static_name` would be its first frame.
    name: String,
}

fn seven_tv(set: SevenTvSet) -> impl Iterator<Item = (String, String)> {
    set.emotes.into_iter().filter_map(|e| {
        let file = e.data.host.files.iter().find(|f| f.name == "2x.webp")?;
        Some((e.name, format!("https:{}/{}", e.data.host.url, file.name)))
    })
}

#[derive(Deserialize)]
struct Bttv {
    id: String,
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BttvUser {
    #[serde(default)]
    channel_emotes: Vec<Bttv>,
    #[serde(default)]
    shared_emotes: Vec<Bttv>,
}

fn bttv(list: Vec<Bttv>) -> impl Iterator<Item = (String, String)> {
    list.into_iter().map(|e| (e.code, format!("https://cdn.betterttv.net/emote/{}/2x", e.id)))
}

#[derive(Deserialize)]
struct Ffz {
    code: String,
    images: HashMap<String, Option<String>>,
}

fn ffz(list: Vec<Ffz>) -> impl Iterator<Item = (String, String)> {
    list.into_iter().filter_map(|mut e| {
        let url = e.images.remove("2x").flatten().or(e.images.remove("1x").flatten())?;
        Some((e.code, url))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse<T: DeserializeOwned>(json: &str) -> T {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn seven_tv_uses_animated_2x() {
        let user: SevenTvUser = parse(include_str!("../../fixtures/7tv_user.json"));
        let map: Emotes = seven_tv(user.emote_set.unwrap()).collect();
        assert_eq!(map["OMEGALUL"], "https://cdn.7tv.app/emote/e1/2x.webp");
        assert_eq!(map["catJAM"], "https://cdn.7tv.app/emote/e2/2x.webp");
    }

    #[test]
    fn bttv_global_and_user() {
        let global: Vec<Bttv> = parse(include_str!("../../fixtures/bttv_global.json"));
        let user: BttvUser = parse(include_str!("../../fixtures/bttv_user.json"));
        let map: Emotes = bttv(global).chain(bttv(user.shared_emotes)).chain(bttv(user.channel_emotes)).collect();
        assert_eq!(map[":tf:"], "https://cdn.betterttv.net/emote/b2/2x");
        assert_eq!(map["monkaS"], "https://cdn.betterttv.net/emote/b3/2x");
        assert_eq!(map["PepeHands"], "https://cdn.betterttv.net/emote/b4/2x");
    }

    #[test]
    fn ffz_falls_back_to_1x() {
        let list: Vec<Ffz> = parse(include_str!("../../fixtures/ffz.json"));
        let map: Emotes = ffz(list).collect();
        assert_eq!(map["ZrehplaR"], "https://cdn.betterttv.net/frankerfacez_emote/9/2");
        assert_eq!(map["Small"], "https://cdn.betterttv.net/frankerfacez_emote/10/1");
    }

    #[test]
    fn seven_tv_user_without_set() {
        let user: SevenTvUser = parse(r#"{"id":"u","emote_set":null}"#);
        assert!(user.emote_set.is_none());
    }
}
