//! Self-update from GitHub releases. Our builds are a portable `.exe` and a single
//! AppImage, which `tauri-plugin-updater` (installers only on Windows) cannot update.
//!
//! The release workflow publishes fixed asset names and a `SHA256SUMS` file
//! (`.github/workflows/release.yml`); this module relies on both.

use std::io::Write;
use std::path::{Path, PathBuf};

use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::failure::{self, Kind};

const LATEST_URL: &str = "https://api.github.com/repos/WahhaWood/wclip/releases/latest";
const SUMS: &str = "SHA256SUMS";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("release version: {0}")]
    Version(#[from] semver::Error),
    #[error("{0}")]
    Release(String),
    #[error("{0}: checksum does not match SHA256SUMS")]
    Checksum(String),
}

impl Error {
    pub fn kind(&self) -> Kind {
        match self {
            Error::Http(e) if e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() => Kind::Network,
            Error::Io(e) => failure::io_kind(e),
            _ => Kind::Other,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A release newer than the running app.
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// Asset of this platform and the checksum file, when the release has them.
    files: Option<(String, String)>,
}

/// What the UI shows about a [`Release`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub version: String,
    pub notes: String,
    pub page: String,
    /// The running copy can replace itself; otherwise the UI links to `page`.
    pub installable: bool,
}

impl Release {
    pub fn info(&self) -> Info {
        Info {
            version: self.version.clone(),
            notes: self.notes.clone(),
            page: self.page.clone(),
            installable: self.files.is_some() && target().is_some(),
        }
    }
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

/// Asset name of this platform in a release.
fn asset_name() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("WClip-windows-x64.exe"),
        ("linux", "x86_64") => Some("WClip-linux-x86_64.AppImage"),
        _ => None,
    }
}

/// The file to replace: the `.exe` itself, or the AppImage it runs from. A Linux
/// build run any other way (dev, unpacked) cannot update itself.
pub fn target() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::current_exe().ok();
    }
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file())
}

/// The latest release if it is newer than this build. `releases/latest` already
/// leaves out drafts and pre-releases.
pub async fn latest(client: &reqwest::Client) -> Result<Option<Release>> {
    let resp = client
        .get(LATEST_URL)
        .header("User-Agent", concat!("WClip/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?;
    // No release published yet.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let gh: GhRelease = resp.error_for_status()?.json().await?;
    if !is_newer(&gh.tag_name, env!("CARGO_PKG_VERSION"))? {
        return Ok(None);
    }
    let url = |name: &str| gh.assets.iter().find(|a| a.name == name).map(|a| a.browser_download_url.clone());
    let files = asset_name().and_then(|name| Some((url(name)?, url(SUMS)?)));
    Ok(Some(Release {
        version: gh.tag_name.trim_start_matches('v').to_owned(),
        notes: gh.body.unwrap_or_default(),
        page: gh.html_url,
        files,
    }))
}

fn is_newer(tag: &str, current: &str) -> Result<bool> {
    let tag = Version::parse(tag.trim_start_matches('v'))?;
    Ok(tag.pre.is_empty() && tag > Version::parse(current)?)
}

/// Hash of `name` in a `sha256sum` listing (`<hex>  <name>`, or `<hex> *<name>`).
fn checksum<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name).then_some(hash)
    })
}

/// Downloads the release next to `target` (see [`target`]), checks it against
/// `SHA256SUMS` and puts it in place of `target`.
pub async fn install(
    client: &reqwest::Client,
    release: &Release,
    target: &Path,
    on_progress: impl Fn(u64, Option<u64>),
) -> Result<()> {
    let not_installable = || Error::Release("this copy cannot update itself".into());
    let (asset_url, sums_url) = release.files.as_ref().ok_or_else(not_installable)?;
    let name = asset_name().ok_or_else(not_installable)?;

    let sums = client.get(sums_url).send().await?.error_for_status()?.text().await?;
    let expected = checksum(&sums, name).ok_or_else(|| Error::Release(format!("{name} is not in {SUMS}")))?;

    // Same directory as the target, so the final rename never crosses file systems.
    let part = PartFile(target.with_file_name(".WClip-update.part"));
    let mut resp = client.get(asset_url).send().await?.error_for_status()?;
    let total = resp.content_length();
    let mut file = std::fs::File::create(&part.0)?;
    let mut hasher = Sha256::new();
    let mut done = 0;
    on_progress(done, total);
    while let Some(chunk) = resp.chunk().await? {
        file.write_all(&chunk)?;
        hasher.update(&chunk);
        done += chunk.len() as u64;
        on_progress(done, total);
    }
    file.flush()?;
    drop(file);
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(Error::Checksum(name.to_owned()));
    }
    replace(&part.0, target)
}

/// A running `.exe` cannot be overwritten, `self-replace` moves it aside first.
#[cfg(windows)]
fn replace(new: &Path, _target: &Path) -> Result<()> {
    self_replace::self_replace(new)?;
    Ok(())
}

/// The running AppImage stays mounted from the old inode, so a rename is safe.
#[cfg(not(windows))]
fn replace(new: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(new, target)?;
    Ok(())
}

/// Starts the updated copy; the caller then exits.
pub fn launch(target: &Path) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(target);
    // The AppImage runtime points variables like LD_LIBRARY_PATH into its mount,
    // which goes away with this process.
    if let Some(appdir) = std::env::var_os("APPDIR").filter(|d| !d.is_empty()) {
        let appdir = appdir.to_string_lossy().into_owned();
        for (key, value) in std::env::vars_os() {
            if value.to_string_lossy().contains(&appdir) {
                cmd.env_remove(key);
            }
        }
        cmd.env_remove("APPIMAGE").env_remove("APPDIR").env_remove("ARGV0").env_remove("OWD");
    }
    cmd.spawn().map(|_| ())
}

/// Removed on drop unless renamed away first.
struct PartFile(PathBuf);

impl Drop for PartFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.1.1", "0.1.0").unwrap());
        assert!(is_newer("0.2.0", "0.1.9").unwrap());
        assert!(is_newer("v0.10.0", "0.9.0").unwrap(), "numeric, not text");
        assert!(!is_newer("v0.1.0", "0.1.0").unwrap());
        assert!(!is_newer("v0.0.9", "0.1.0").unwrap());
        assert!(!is_newer("v0.2.0-beta.1", "0.1.0").unwrap(), "pre-release");
        assert!(is_newer("nightly", "0.1.0").is_err());
    }

    #[test]
    fn sums() {
        let sums = "aa11  WClip-linux-x86_64.AppImage\nbb22 *WClip-windows-x64.exe\n\n";
        assert_eq!(checksum(sums, "WClip-linux-x86_64.AppImage"), Some("aa11"));
        assert_eq!(checksum(sums, "WClip-windows-x64.exe"), Some("bb22"));
        assert_eq!(checksum(sums, "WClip-linux"), None);
    }

    #[test]
    fn release_json() {
        let gh: GhRelease = serde_json::from_str(
            r#"{"tag_name":"v0.1.1","body":null,"html_url":"https://github.com/x","draft":false,
                "assets":[{"name":"SHA256SUMS","browser_download_url":"https://d/s","size":1}]}"#,
        )
        .unwrap();
        assert_eq!(gh.assets.len(), 1);
        assert!(gh.body.is_none());
    }

    /// Serves `files` (path, body) over HTTP on localhost, one request per connection.
    fn serve(files: Vec<(&'static str, Vec<u8>)>) -> String {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request = String::new();
                BufReader::new(&stream).read_line(&mut request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let body = files.iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone()).unwrap_or_default();
                let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(&body));
            }
        });
        format!("http://{addr}")
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[tokio::test]
    async fn install_checks_and_replaces() {
        let new = b"new build".to_vec();
        let hash: String = Sha256::digest(&new).iter().map(|b| format!("{b:02x}")).collect();
        let base = serve(vec![
            ("/app", new.clone()),
            ("/sums", format!("{hash}  WClip-linux-x86_64.AppImage\n").into_bytes()),
            ("/bad-sums", b"00ff  WClip-linux-x86_64.AppImage\n".to_vec()),
        ]);
        let release = |sums: &str| Release {
            version: "9.0.0".into(),
            notes: String::new(),
            page: String::new(),
            files: Some((format!("{base}/app"), format!("{base}{sums}"))),
        };
        let dir = std::env::temp_dir().join(format!("wclip-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("WClip.AppImage");
        std::fs::write(&target, b"old build").unwrap();
        let client = reqwest::Client::new();

        let err = install(&client, &release("/bad-sums"), &target, |_, _| {}).await.unwrap_err();
        assert!(matches!(err, Error::Checksum(_)), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"old build");

        install(&client, &release("/sums"), &target, |_, _| {}).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), new);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no leftover part file");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Real GitHub API: `cargo test -- --ignored smoke_latest`.
    #[tokio::test]
    #[ignore]
    async fn smoke_latest() {
        let release = latest(&reqwest::Client::new()).await.unwrap();
        println!("{:?}", release.map(|r| r.info()));
    }
}
