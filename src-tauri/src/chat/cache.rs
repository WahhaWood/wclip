//! Whole-VOD chat kept on disk, so a VOD opened again skips the long download.
//! Raw comments are kept, not messages: badges and emotes are fetched on every
//! load, so a provider that failed once does not stay missing.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};

use crate::twitch::chat::Comment;

/// VODs kept; the least recently opened go first.
const KEEP: usize = 20;
/// Bumped when `Comment` changes, so older files are downloaded again.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Saved<C> {
    version: u32,
    /// Length of the VOD when saved.
    duration: f64,
    comments: C,
}

fn path(dir: &Path, vod_id: &str) -> PathBuf {
    dir.join(format!("{vod_id}.json.gz"))
}

/// Comments saved for `vod_id` if they cover `duration` seconds: the VOD of a live
/// stream grows.
pub fn read(dir: &Path, vod_id: &str, duration: f64) -> Option<Vec<Comment>> {
    let path = path(dir, vod_id);
    let file = File::open(&path).ok()?;
    let saved: Saved<Vec<Comment>> = serde_json::from_reader(BufReader::new(GzDecoder::new(file))).ok()?;
    if saved.version != VERSION || saved.duration < duration {
        return None;
    }
    // Recently used for `prune`. Windows needs write access to set the time.
    if let Ok(file) = OpenOptions::new().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(saved.comments)
}

/// Saves `comments` of a VOD `duration` long and drops the oldest files over `KEEP`.
pub fn write(dir: &Path, vod_id: &str, duration: f64, comments: &[Comment]) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = path(dir, vod_id);
    // Written aside first: a half-written file is never read.
    let part = path.with_extension("part");
    let mut out = GzEncoder::new(BufWriter::new(File::create(&part)?), Compression::fast());
    serde_json::to_writer(&mut out, &Saved { version: VERSION, duration, comments })?;
    out.finish()?.flush()?;
    std::fs::rename(&part, &path)?;
    prune(dir);
    Ok(())
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json.gz"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    for (_, path) in files.iter().skip(KEEP) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::twitch::chat::{Commenter, Fragment, Message};

    fn comment(id: &str, time: f64) -> Comment {
        Comment {
            id: id.to_owned(),
            content_offset_seconds: time,
            commenter: Some(Commenter { login: "alice".into(), display_name: "Alice".into() }),
            message: Message {
                fragments: vec![Fragment { text: "hi".into(), emote: None }],
                user_badges: Vec::new(),
                user_color: None,
            },
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wclip-test-chat-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn reads_back_what_covers_the_vod() {
        let dir = temp_dir("read");
        write(&dir, "1", 100.0, &[comment("a", 1.0), comment("b", 50.0)]).unwrap();
        let back = read(&dir, "1", 100.0).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].id, "b");
        assert_eq!(back[1].content_offset_seconds, 50.0);
        assert_eq!(back[0].commenter.as_ref().unwrap().display_name, "Alice");
        // Saved while the stream was live: shorter than the VOD is now.
        assert!(read(&dir, "1", 120.0).is_none());
        assert!(read(&dir, "2", 100.0).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keeps_recent_vods() {
        let dir = temp_dir("prune");
        for i in 0..KEEP {
            write(&dir, &i.to_string(), 10.0, &[comment("a", 1.0)]).unwrap();
            let time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i as u64);
            OpenOptions::new().write(true).open(path(&dir, &i.to_string())).unwrap().set_modified(time).unwrap();
        }
        // Opening the oldest makes it recent, so the next oldest goes.
        assert!(read(&dir, "0", 10.0).is_some());
        write(&dir, "new", 10.0, &[]).unwrap();
        assert!(read(&dir, "1", 10.0).is_none());
        for kept in ["0", "2", "new"] {
            assert!(read(&dir, kept, 10.0).is_some(), "{kept}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
