//! All Twitch network access lives here (GQL, HLS).

pub mod channel;
pub mod chat;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

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

#[derive(Deserialize)]
struct GqlError {
    message: String,
}

#[derive(Deserialize)]
struct QueryResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GqlError>,
}

/// Plain (not persisted) GQL query; these need no integrity token.
async fn query<T: DeserializeOwned>(
    client: &reqwest::Client,
    query: &str,
    variables: serde_json::Value,
) -> Result<T> {
    let response: QueryResponse<T> = client
        .post(GQL_URL)
        .header("Client-ID", CLIENT_ID)
        .json(&json!({ "query": query, "variables": variables }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(err) = response.errors.into_iter().next() {
        return Err(Error::Gql(err.message));
    }
    response.data.ok_or_else(|| Error::Gql("empty response".into()))
}
