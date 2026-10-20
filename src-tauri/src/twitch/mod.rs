//! All Twitch network access lives here (GQL, HLS).

pub mod channel;
pub mod chat;

use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::failure::Kind;

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

impl Error {
    pub fn kind(&self) -> Kind {
        match self {
            Error::Http(e) if is_transient(e) => Kind::Network,
            Error::VideoNotFound => Kind::VodMissing,
            _ => Kind::Other,
        }
    }
}

/// Worth retrying: no answer, a cut-off answer or a server error.
fn is_transient(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() || e.status().is_some_and(|s| s.is_server_error())
}

/// Attempts per GQL request; waits 1 s, then 2 s between them.
const ATTEMPTS: u32 = 3;

/// POSTs `body` to GQL, retrying transient failures.
async fn post<T: DeserializeOwned>(client: &reqwest::Client, body: &serde_json::Value) -> Result<T> {
    let mut attempt = 1;
    loop {
        let result = async {
            client
                .post(GQL_URL)
                .header("Client-ID", CLIENT_ID)
                .json(body)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
        }
        .await;
        match result {
            Err(e) if attempt < ATTEMPTS && is_transient(&e) => {
                tokio::time::sleep(Duration::from_secs(attempt.into())).await;
                attempt += 1;
            }
            result => return Ok(result?),
        }
    }
}

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
    let response: QueryResponse<T> = post(client, &json!({ "query": query, "variables": variables })).await?;
    if let Some(err) = response.errors.into_iter().next() {
        return Err(Error::Gql(err.message));
    }
    response.data.ok_or_else(|| Error::Gql("empty response".into()))
}
