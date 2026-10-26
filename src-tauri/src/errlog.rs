//! Error log: one line per failure and panic, for the user to attach to a bug report.
//! URLs lose their query string, where Twitch puts playback tokens.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Past this the log moves to `<name>.old`, replacing the previous one.
const MAX_SIZE: u64 = 512 * 1024;

static PATH: OnceLock<PathBuf> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());
/// The Twitch token from the settings: never written to the log, whatever the message holds.
static SECRET: Mutex<Option<String>> = Mutex::new(None);
/// The first line of each run says which build wrote it.
static HEADER_WRITTEN: AtomicBool = AtomicBool::new(false);

/// Sets the log file and logs panics; call once on start.
pub fn init(path: PathBuf) {
    if PATH.set(path).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write("panic", &info.to_string());
        previous(info);
    }));
}

/// The log file, if anything was ever written to it.
pub fn path() -> Option<&'static Path> {
    PATH.get().map(PathBuf::as_path).filter(|p| p.exists())
}

pub fn set_secret(secret: Option<&str>) {
    *SECRET.lock().unwrap_or_else(|e| e.into_inner()) = secret.map(str::to_owned);
}

pub fn write(context: &str, message: &str) {
    let Some(path) = PATH.get() else { return };
    // A panic while holding the lock poisons it; logging must go on.
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let secret = SECRET.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let message = scrub(&redact(message), &[secret.as_deref().unwrap_or("")]);
    let _ = append(path, &format!("{} {context}: {message}\n", timestamp()));
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_SIZE) {
        std::fs::rename(path, path.with_extension("log.old"))?;
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if !HEADER_WRITTEN.swap(true, Ordering::Relaxed) {
        let (version, os, arch) = (env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
        writeln!(file, "--- WClip {version}, {os} {arch}")?;
    }
    file.write_all(line.as_bytes())
}

/// The last `n` lines of the log, empty if there is none.
pub fn tail(n: usize) -> String {
    let Some(path) = path() else { return String::new() };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `text` without any of the non-empty `secrets` (a Twitch token, say).
pub fn scrub(text: &str, secrets: &[&str]) -> String {
    secrets.iter().filter(|s| !s.trim().is_empty()).fold(text.to_owned(), |t, s| t.replace(s.trim(), "…"))
}

/// `text` with the query string of every URL replaced by `?…`, and on one line.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        let (before, url) = rest.split_at(at);
        out.push_str(before);
        let end = url.find(|c: char| c.is_whitespace() || "\"'<>()".contains(c)).unwrap_or(url.len());
        let url_only = &url[..end];
        match url_only.find('?') {
            Some(q) => {
                out.push_str(&url_only[..q]);
                out.push_str("?…");
            }
            None => out.push_str(url_only),
        }
        rest = &url[end..];
    }
    out.push_str(rest);
    out.replace(['\n', '\r'], " ")
}

/// UTC, `2026-10-20 16:05:03`.
fn timestamp() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let t = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

/// Date of a day number since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_url_queries() {
        let text = "ffmpeg: [https @ 0x1] HTTP error 403 for https://usher.ttvnw.net/vod/1.m3u8?sig=abc&token=%7B%22a%22 (x)\nnext";
        assert_eq!(
            redact(text),
            "ffmpeg: [https @ 0x1] HTTP error 403 for https://usher.ttvnw.net/vod/1.m3u8?… (x) next"
        );
        assert_eq!(redact("https://cdn/x/index-dvr.m3u8 ok"), "https://cdn/x/index-dvr.m3u8 ok");
        assert_eq!(redact("no links"), "no links");
    }

    #[test]
    fn scrubs_secrets() {
        assert_eq!(scrub("a tok123 b tok123", &["tok123"]), "a … b …");
        assert_eq!(scrub("keep", &["", "  "]), "keep");
    }

    #[test]
    fn token_stays_out_of_the_log() {
        let dir = std::env::temp_dir().join(format!("wclip-log-secret-{}", std::process::id()));
        let path = dir.join("wclip.log");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = PATH.set(path.clone());
        let Some(path) = PATH.get().cloned() else { return };
        set_secret(Some("secrettoken123"));
        write("vod", "yt-dlp failed with cookie secrettoken123 here");
        set_secret(None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("yt-dlp failed") && !text.contains("secrettoken123"), "{text}");
    }

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_746), (2026, 10, 20));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn writes_and_rotates() {
        let dir = std::env::temp_dir().join(format!("wclip-log-{}", std::process::id()));
        let path = dir.join("wclip.log");
        append(&path, "one\n").unwrap();
        std::fs::write(&path, vec![b'x'; MAX_SIZE as usize + 1]).unwrap();
        append(&path, "two\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two\n");
        assert!(path.with_extension("log.old").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
