//! Errors as the UI shows them: a kind it can explain in words, plus the raw text.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// Deleted, private or never existed: Twitch answers the same for all three.
    VodMissing,
    SubOnly,
    Network,
    DiskFull,
    /// The chat could not be loaded; the clip can still be cut without it.
    Chat,
    Other,
}

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub kind: Kind,
    pub detail: String,
}

impl Failure {
    pub fn new(kind: Kind, detail: impl ToString) -> Self {
        Self { kind, detail: detail.to_string() }
    }
}

impl From<String> for Failure {
    fn from(detail: String) -> Self {
        Self { kind: Kind::Other, detail }
    }
}

impl From<&str> for Failure {
    fn from(detail: &str) -> Self {
        detail.to_owned().into()
    }
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Self::new(io_kind(&e), e)
    }
}

pub fn io_kind(e: &std::io::Error) -> Kind {
    match e.kind() {
        std::io::ErrorKind::StorageFull => Kind::DiskFull,
        _ => Kind::Other,
    }
}

/// Kind of an error that only exists as text: yt-dlp and ffmpeg stderr.
pub fn classify(text: &str) -> Kind {
    const NETWORK: &[&str] = &[
        "unable to download",
        "failed to resolve",
        "name or service not known",
        "temporary failure in name resolution",
        "no such host",
        "connection refused",
        "connection reset",
        "timed out",
        "network is unreachable",
        "i/o error",
        "input/output error",
        "error in the pull function",
        "server returned 5",
    ];
    let text = text.to_lowercase();
    if text.contains("does not exist") {
        Kind::VodMissing
    } else if text.contains("subscriber-only") {
        Kind::SubOnly
    } else if text.contains("no space left on device") {
        Kind::DiskFull
    } else if NETWORK.iter().any(|p| text.contains(p)) {
        Kind::Network
    } else {
        Kind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_messages() {
        let cases = [
            ("yt-dlp: [twitch:vod] 1: Video 1 does not exist", Kind::VodMissing),
            (
                "yt-dlp: [twitch:vod] v2: You must be logged into an account that has access to this subscriber-only content",
                Kind::SubOnly,
            ),
            ("yt-dlp: [twitch:vod] v2: Unable to download JSON metadata: <urlopen error [Errno -3] Temporary failure in name resolution>", Kind::Network),
            ("ffmpeg: [out#0/mp4 @ 0x5] Error writing trailer: No space left on device", Kind::DiskFull),
            ("ffmpeg: [tls @ 0x5] Error in the pull function.", Kind::Network),
            ("ffmpeg: [https @ 0x5] Connection timed out", Kind::Network),
            ("ffmpeg: Unknown encoder 'libx264'", Kind::Other),
        ];
        for (text, kind) in cases {
            assert_eq!(classify(text), kind, "{text}");
        }
    }
}
