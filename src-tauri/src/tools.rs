//! External tools: ffmpeg, ffprobe and yt-dlp (ADR 0002).
//!
//! Lookup order is our tools directory first, then `PATH`; a candidate counts only if
//! its version command runs. Missing tools are downloaded into the tools directory:
//! ffmpeg and ffprobe from BtbN static builds, yt-dlp from its GitHub releases.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use sha2::{Digest, Sha256};

#[cfg(windows)]
const FFMPEG_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip";
#[cfg(not(windows))]
const FFMPEG_URL: &str =
    "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz";
#[cfg(windows)]
const YT_DLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
#[cfg(not(windows))]
const YT_DLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_linux";
/// `sha256sum` listings published next to the downloads. They come from the same
/// release, so they catch a broken download, not a tampered release.
const FFMPEG_SUMS_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/checksums.sha256";
const YT_DLP_SUMS_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";

/// Touched after every self-update attempt of our yt-dlp copy.
const YT_DLP_STAMP: &str = "yt-dlp.checked";
const YT_DLP_UPDATE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("archive: {0}")]
    Archive(String),
    #[error("{0} does not run after download")]
    Broken(&'static str),
    #[error("{0}: checksum does not match, try again")]
    Checksum(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
    YtDlp,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Ffmpeg, Tool::Ffprobe, Tool::YtDlp];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::YtDlp => "yt-dlp",
        }
    }

    fn file_name(self) -> String {
        format!("{}{}", self.name(), std::env::consts::EXE_SUFFIX)
    }

    fn version_arg(self) -> &'static str {
        match self {
            Tool::YtDlp => "--version",
            _ => "-version",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolInfo {
    pub tool: Tool,
    /// `None` when the tool is found nowhere.
    pub path: Option<PathBuf>,
    /// First line of the version output.
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Download unit: `Ffmpeg` also brings ffprobe.
    pub tool: Tool,
    pub done: u64,
    pub total: Option<u64>,
}

/// Resolved paths of all tools, ready to run.
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub yt_dlp: PathBuf,
}

impl Tools {
    pub fn from_status(status: &[ToolInfo]) -> Option<Tools> {
        let path = |tool| status.iter().find(|i| i.tool == tool)?.path.clone();
        Some(Tools {
            ffmpeg: path(Tool::Ffmpeg)?,
            ffprobe: path(Tool::Ffprobe)?,
            yt_dlp: path(Tool::YtDlp)?,
        })
    }
}

/// `Command` that does not flash a console window on Windows.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

pub fn status(dir: &Path) -> Vec<ToolInfo> {
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    Tool::ALL
        .into_iter()
        .map(|tool| {
            let found = candidates(tool, dir, &path_dirs)
                .into_iter()
                .find_map(|path| version(tool, &path).map(|v| (path, v)));
            ToolInfo {
                tool,
                version: found.as_ref().map(|(_, v)| v.clone()),
                path: found.map(|(p, _)| p),
            }
        })
        .collect()
}

fn candidates(tool: Tool, dir: &Path, path_dirs: &[PathBuf]) -> Vec<PathBuf> {
    std::iter::once(dir)
        .chain(path_dirs.iter().map(PathBuf::as_path))
        .map(|d| d.join(tool.file_name()))
        .filter(|p| p.is_file())
        .collect()
}

fn version(tool: Tool, path: &Path) -> Option<String> {
    let out = command(path).arg(tool.version_arg()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Some(text.lines().next().unwrap_or_default().trim().to_owned())
}

/// Downloads every tool missing from `status` into `dir`.
pub async fn install_missing(
    dir: &Path,
    status: &[ToolInfo],
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<()> {
    let missing = |tool| status.iter().any(|i| i.tool == tool && i.path.is_none());
    std::fs::create_dir_all(dir)?;
    let client = reqwest::Client::new();

    if missing(Tool::YtDlp) {
        let target = dir.join(Tool::YtDlp.file_name());
        download(&client, YT_DLP_URL, YT_DLP_SUMS_URL, &target, Tool::YtDlp, on_progress).await?;
        make_executable(&target)?;
        version(Tool::YtDlp, &target).ok_or(Error::Broken("yt-dlp"))?;
    }

    if missing(Tool::Ffmpeg) || missing(Tool::Ffprobe) {
        let archive = dir.join(if cfg!(windows) { "ffmpeg.zip" } else { "ffmpeg.tar.xz" });
        download(&client, FFMPEG_URL, FFMPEG_SUMS_URL, &archive, Tool::Ffmpeg, on_progress).await?;
        let (archive_path, dir_path) = (archive.clone(), dir.to_owned());
        let extracted = tauri::async_runtime::spawn_blocking(move || {
            extract_ffmpeg(&archive_path, &dir_path)
        })
        .await
        .map_err(|e| Error::Archive(e.to_string()))?;
        std::fs::remove_file(&archive)?;
        extracted?;
        for tool in [Tool::Ffmpeg, Tool::Ffprobe] {
            let target = dir.join(tool.file_name());
            make_executable(&target)?;
            version(tool, &target).ok_or(Error::Broken(tool.name()))?;
        }
    }
    Ok(())
}

/// Streams `url` into `target` through a `.part` file, so a broken download never
/// looks like an installed tool, and checks it against the `sums_url` listing.
async fn download(
    client: &reqwest::Client,
    url: &str,
    sums_url: &str,
    target: &Path,
    tool: Tool,
    on_progress: &(dyn Fn(Progress) + Sync),
) -> Result<()> {
    let name = url.rsplit('/').next().unwrap_or(url);
    let sums = client.get(sums_url).send().await?.error_for_status()?.text().await?;
    let expected = crate::updater::checksum(&sums, name).ok_or_else(|| Error::Checksum(name.to_owned()))?;
    download_checked(client, url, expected, target, &|done, total| on_progress(Progress { tool, done, total })).await
}

/// Downloads `url` to `target` through a `.part` file and keeps it only if its
/// SHA-256 is `expected` (hex). `on_bytes` gets the bytes done and the total.
pub(crate) async fn download_checked(
    client: &reqwest::Client,
    url: &str,
    expected: &str,
    target: &Path,
    on_bytes: &(dyn Fn(u64, Option<u64>) + Sync),
) -> Result<()> {
    let name = url.rsplit('/').next().unwrap_or(url);
    let mut resp = client.get(url).send().await?.error_for_status()?;
    let total = resp.content_length();
    let part = target.with_extension("part");
    let mut file = std::fs::File::create(&part)?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    on_bytes(done, total);
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        done += chunk.len() as u64;
        on_bytes(done, total);
    }
    file.flush()?;
    drop(file);
    if !hex(&hasher.finalize()).eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Checksum(name.to_owned()));
    }
    std::fs::rename(&part, target)?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Pulls `bin/ffmpeg` and `bin/ffprobe` out of a BtbN archive into `dir`.
fn extract_ffmpeg(archive: &Path, dir: &Path) -> Result<()> {
    if cfg!(windows) {
        extract_from_zip(archive, dir, &[Tool::Ffmpeg, Tool::Ffprobe])
    } else {
        extract_from_tar(archive, dir, &[Tool::Ffmpeg, Tool::Ffprobe])
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn extract_from_zip(archive: &Path, dir: &Path, tools: &[Tool]) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)
        .map_err(|e| Error::Archive(e.to_string()))?;
    for &tool in tools {
        let suffix = format!("/bin/{}", tool.file_name());
        let name = zip
            .file_names()
            .filter_map(|n| n.ok())
            .find(|n| n.ends_with(&suffix))
            .ok_or_else(|| Error::Archive(format!("no {suffix}")))?
            .into_owned();
        let mut entry = zip.by_name(&name).map_err(|e| Error::Archive(e.to_string()))?;
        let mut out = std::fs::File::create(dir.join(tool.file_name()))?;
        io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

/// Uses the system `tar`: every Linux has one, and it handles `.xz`.
#[cfg_attr(windows, allow(dead_code))]
fn extract_from_tar(archive: &Path, dir: &Path, tools: &[Tool]) -> Result<()> {
    let out = command(Path::new("tar"))
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .args(["--strip-components=2", "--wildcards"])
        .args(tools.iter().map(|t| format!("*/bin/{}", t.file_name())))
        .output()?;
    if !out.status.success() {
        return Err(Error::Archive(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
    }
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Runs `yt-dlp -U` on our own copy at most once a day. A yt-dlp from `PATH` belongs
/// to the system package manager and is left alone. Errors are ignored: an old
/// yt-dlp is still better than none.
pub fn update_yt_dlp(dir: &Path) {
    let own = dir.join(Tool::YtDlp.file_name());
    let stamp = dir.join(YT_DLP_STAMP);
    if !own.is_file() || !update_due(&stamp, SystemTime::now()) {
        return;
    }
    let _ = command(&own).arg("-U").output();
    let _ = std::fs::write(&stamp, b"");
}

/// Whether a day has passed since `stamp` was last written (or it does not exist).
pub(crate) fn update_due(stamp: &Path, now: SystemTime) -> bool {
    let Ok(modified) = std::fs::metadata(stamp).and_then(|m| m.modified()) else {
        return true;
    };
    now.duration_since(modified).map_or(true, |age| age >= YT_DLP_UPDATE_EVERY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wclip-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn download_checks_sum() {
        let body = b"tool".to_vec();
        let hash = hex(&Sha256::digest(&body));
        let base = crate::updater::tests::serve(vec![
            ("/dl/yt-dlp_test", body.clone()),
            ("/good", format!("{hash}  yt-dlp_test\n").into_bytes()),
            ("/bad", b"00ff  yt-dlp_test\n".to_vec()),
            ("/other", format!("{hash}  something_else\n").into_bytes()),
        ]);
        let dir = temp_dir("sums");
        let target = dir.join("yt-dlp");
        let client = reqwest::Client::new();
        let url = format!("{base}/dl/yt-dlp_test");
        for sums in ["/bad", "/other"] {
            let err = download(&client, &url, &format!("{base}{sums}"), &target, Tool::YtDlp, &|_| {}).await.unwrap_err();
            assert!(matches!(err, Error::Checksum(_)), "{sums}: {err}");
            assert!(!target.exists() && !target.with_extension("part").exists(), "{sums}");
        }
        download(&client, &url, &format!("{base}/good"), &target, Tool::YtDlp, &|_| {}).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), body);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn own_dir_goes_before_path() {
        let own = temp_dir("own");
        let sys = temp_dir("sys");
        let name = Tool::Ffmpeg.file_name();
        std::fs::write(own.join(&name), b"").unwrap();
        std::fs::write(sys.join(&name), b"").unwrap();
        let found = candidates(Tool::Ffmpeg, &own, &[temp_dir("empty"), sys.clone()]);
        assert_eq!(found, vec![own.join(&name), sys.join(&name)]);
    }

    #[test]
    fn update_due_after_a_day() {
        let dir = temp_dir("stamp");
        let stamp = dir.join(YT_DLP_STAMP);
        let now = SystemTime::now();
        assert!(update_due(&stamp, now), "no stamp yet");
        std::fs::write(&stamp, b"").unwrap();
        assert!(!update_due(&stamp, now + Duration::from_secs(60)));
        assert!(update_due(&stamp, now + YT_DLP_UPDATE_EVERY + Duration::from_secs(60)));
    }

    #[test]
    fn zip_extracts_only_bin_tools() {
        let dir = temp_dir("zip");
        let archive = dir.join("ffmpeg.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("ffmpeg-master/bin/ffplay", "x"),
            ("ffmpeg-master/bin/ffmpeg", "a"),
            ("ffmpeg-master/doc/ffprobe", "x"),
            ("ffmpeg-master/bin/ffprobe", "b"),
        ] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();

        let out = temp_dir("zip-out");
        let tools = [Tool::Ffmpeg, Tool::Ffprobe];
        // Archive entries carry the host's exe suffix, so on Windows this checks "no entry".
        if cfg!(windows) {
            assert!(extract_from_zip(&archive, &out, &tools).is_err());
            return;
        }
        extract_from_zip(&archive, &out, &tools).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("ffmpeg")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(out.join("ffprobe")).unwrap(), "b");
        assert!(!out.join("ffplay").exists());
    }

    #[cfg(unix)]
    #[test]
    fn tar_extracts_only_bin_tools() {
        let src = temp_dir("tar-src");
        let root = src.join("ffmpeg-master");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("doc")).unwrap();
        std::fs::write(root.join("bin/ffmpeg"), "a").unwrap();
        std::fs::write(root.join("bin/ffprobe"), "b").unwrap();
        std::fs::write(root.join("bin/ffplay"), "x").unwrap();
        std::fs::write(root.join("doc/ffmpeg"), "x").unwrap();
        let archive = src.join("ffmpeg.tar.xz");
        let ok = Command::new("tar")
            .arg("-cJf")
            .arg(&archive)
            .arg("-C")
            .arg(&src)
            .arg("ffmpeg-master")
            .status()
            .unwrap()
            .success();
        assert!(ok);

        let out = temp_dir("tar-out");
        extract_from_tar(&archive, &out, &[Tool::Ffmpeg, Tool::Ffprobe]).unwrap();
        assert_eq!(std::fs::read_to_string(out.join("ffmpeg")).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(out.join("ffprobe")).unwrap(), "b");
        assert!(!out.join("ffplay").exists());
    }

    /// Real download of everything (~200 MB): `cargo test -- --ignored install`.
    #[tokio::test]
    #[ignore]
    async fn install_from_network() {
        let dir = temp_dir("install");
        let before = status(&dir);
        let missing: Vec<ToolInfo> = before
            .into_iter()
            .map(|i| ToolInfo { path: None, version: None, ..i })
            .collect();
        install_missing(&dir, &missing, &|_| {}).await.unwrap();
        let after = status(&dir);
        for info in &after {
            assert_eq!(info.path.as_deref(), Some(dir.join(info.tool.file_name()).as_path()));
            println!("{}: {}", info.tool.name(), info.version.as_deref().unwrap_or("?"));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
