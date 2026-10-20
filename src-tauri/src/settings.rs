//! User settings and overlay presets, kept as JSON in the app config directory.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::job::DEFAULT_TEMPLATE;
use crate::overlay::OverlayStyle;

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
    /// Release the user chose to skip; its update banner is not shown again.
    pub skipped_version: Option<String>,
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
            skipped_version: None,
        }
    }
}

/// Missing or unreadable file gives the defaults; fields missing from an older file
/// take their defaults too.
pub fn load(path: &Path) -> Settings {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Written through a temporary file, so a crash never leaves half a file.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings).expect("serializable"))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::Corner;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("wclip-test-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        assert_eq!(load(&path).name_template, DEFAULT_TEMPLATE, "no file yet");

        let mut s = Settings::default();
        s.overlay.corner = Corner::TopRight;
        s.presets.push(Preset { name: "big".into(), style: OverlayStyle { font_size: 30.0, ..OverlayStyle::default() } });
        save(&path, &s).unwrap();
        let back = load(&path);
        assert_eq!(back.overlay.corner, Corner::TopRight);
        assert_eq!(back.presets[0].style.font_size, 30.0);

        std::fs::write(&path, r#"{"chat": false, "overlay": {"width": 300}}"#).unwrap();
        let partial = load(&path);
        assert!(!partial.chat);
        assert_eq!(partial.overlay.width, 300);
        assert_eq!(partial.overlay.height, OverlayStyle::default().height);

        std::fs::write(&path, "not json").unwrap();
        assert!(load(&path).chat);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
