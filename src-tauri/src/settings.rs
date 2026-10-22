//! User settings and overlay presets, kept as JSON in the app config directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::chat::ChatFilter;
use crate::cut::ChatOutput;
use crate::encode::Output;
use crate::job::DEFAULT_TEMPLATE;
use crate::layout::Layout;
use crate::overlay::OverlayStyle;
use crate::subtitles::SubtitleStyle;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// `None` until the user picks one; the app then uses `<Videos>/WClip`.
    pub out_dir: Option<PathBuf>,
    pub name_template: String,
    /// Burn the chat into clips.
    pub chat: bool,
    pub overlay: OverlayStyle,
    pub presets: Vec<Preset>,
    /// Preset picked last; changes to `overlay` go into it too.
    pub active_preset: Option<String>,
    /// Release the user chose to skip; its update banner is not shown again.
    pub skipped_version: Option<String>,
    pub output: Output,
    pub chat_output: ChatOutput,
    /// Also join the clips of a job into one video.
    pub merge: bool,
    pub chat_filter: ChatFilter,
    /// Value of the `auth-token` cookie of twitch.tv, for subscriber-only VODs.
    pub twitch_token: Option<String>,
    /// Cut vertical clips laid out by `layout`.
    pub vertical: bool,
    pub layout: Layout,
    /// Saved layouts.
    pub layouts: Vec<Layout>,
    /// Name of the saved layout picked last for a channel: a streamer keeps the camera in one place.
    pub channel_layouts: HashMap<String, String>,
    /// Burn in subtitles of the speech.
    pub subtitles: bool,
    pub subtitle_style: SubtitleStyle,
    /// Language of the speech, like "ru"; `None` detects it.
    pub speech_language: Option<String>,
    /// Recognise the speech on the GPU when there is one.
    pub speech_gpu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub style: OverlayStyle,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            out_dir: None,
            name_template: DEFAULT_TEMPLATE.to_owned(),
            chat: true,
            overlay: OverlayStyle::default(),
            presets: Vec::new(),
            active_preset: None,
            skipped_version: None,
            output: Output::default(),
            chat_output: ChatOutput::default(),
            merge: false,
            chat_filter: ChatFilter::default(),
            twitch_token: None,
            vertical: false,
            layout: Layout::default(),
            layouts: Vec::new(),
            channel_layouts: HashMap::new(),
            subtitles: false,
            subtitle_style: SubtitleStyle::default(),
            speech_language: None,
            speech_gpu: true,
        }
    }
}

/// Missing or unreadable file gives the defaults; fields missing from an older file
/// take their defaults too.
pub fn load(path: &Path) -> Settings {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Written through a temporary file, so a crash never leaves half a file. Only the
/// user can read it on Unix: it may hold the Twitch token.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    std::io::Write::write_all(&mut crate::private_file(&tmp)?, &serde_json::to_vec_pretty(settings).expect("serializable"))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("wclip-test-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        assert_eq!(load(&path).name_template, DEFAULT_TEMPLATE, "no file yet");

        let mut s = Settings::default();
        s.overlay.x = 1400;
        s.presets.push(Preset { name: "big".into(), style: OverlayStyle { font_size: 30.0, ..OverlayStyle::default() } });
        s.active_preset = Some("big".into());
        save(&path, &s).unwrap();
        let back = load(&path);
        assert_eq!(back.overlay.x, 1400);
        assert_eq!(back.presets[0].style.font_size, 30.0);
        assert_eq!(back.active_preset.as_deref(), Some("big"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        std::fs::write(&path, r#"{"chat": false, "overlay": {"width": 300}, "output": {"codec": "hevc"}, "chatOutput": "both"}"#).unwrap();
        let partial = load(&path);
        assert!(!partial.chat);
        assert_eq!(partial.overlay.width, 300);
        assert_eq!(partial.overlay.height, OverlayStyle::default().height);
        assert_eq!(partial.output.codec, crate::encode::Codec::Hevc);
        assert_eq!(partial.output.encoder, crate::encode::Encoder::Auto);
        assert_eq!(partial.chat_output, ChatOutput::Both);

        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).chat);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
