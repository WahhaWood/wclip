//! The unfinished work (video, intervals, edited subtitles) kept as JSON next to the
//! settings, so a crash or a restart does not lose it. The UI owns the shape.

use std::path::Path;

use serde_json::Value;

/// A missing or unreadable file means no session.
pub fn load(path: &Path) -> Option<Value> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

/// Written through a temporary file, like the settings, and private on Unix.
pub fn save(path: &Path, session: &Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    std::io::Write::write_all(&mut crate::private_file(&tmp)?, &serde_json::to_vec(session).expect("serializable"))?;
    std::fs::rename(&tmp, path)
}

pub fn clear(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_clear() {
        let dir = std::env::temp_dir().join(format!("wclip-test-session-{}", std::process::id()));
        let path = dir.join("session.json");
        assert!(load(&path).is_none(), "no file yet");
        clear(&path).unwrap();

        let session = serde_json::json!({ "input": "123", "rows": [{ "start": "0:10" }] });
        save(&path, &session).unwrap();
        assert_eq!(load(&path), Some(session));
        clear(&path).unwrap();
        assert!(load(&path).is_none());
        std::fs::write(&path, b"{ broken").unwrap();
        assert!(load(&path).is_none(), "unreadable file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
