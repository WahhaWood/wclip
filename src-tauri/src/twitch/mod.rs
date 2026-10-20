//! All Twitch network access lives here (GQL, HLS).

pub mod chat;

/// Public web client id used by twitch.tv itself (same as TwitchDownloader).
const CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const GQL_URL: &str = "https://gql.twitch.tv/gql";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("twitch: {0}")]
    Gql(String),
    #[error("VOD not found or not public")]
    VideoNotFound,
}

pub type Result<T> = std::result::Result<T, Error>;
