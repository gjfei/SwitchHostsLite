//! Update checks and self-updates from the GitHub releases of this app.
//!
//! Every release carries a `latest.json` manifest naming one package per
//! platform with its size and SHA-256 (see `.github/workflows/release.yml`).
//! An update downloads that package and verifies it, installs it over the
//! running app (a `.app` bundle on macOS, the `-setup.exe` installer on
//! Windows) and restarts.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    progress::Progress,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, Global, InteractiveElement as _, ParentElement as _, Styled as _, Window, div, px};
use rust_i18n::t;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::i18n::{Localize as _, LocalizedError, error_text};
use crate::settings::Settings;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const STARTUP_DELAY: Duration = Duration::from_secs(5);
const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;
/// How often download progress is redrawn.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MANIFEST_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_NOTES_CHARS: usize = 4000;
#[cfg(target_os = "windows")]
const PACKAGE_BINARY: &str = "switch-hosts-lite.exe";

// -- update source -------------------------------------------------------------

/// Where releases are looked up.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Source {
    manifest_url: String,
    page_url: String,
}

/// `owner/repo` from a GitHub URL, `None` for anything else or for the
/// `OWNER` placeholder that ships in Cargo.toml.
fn parse_repo(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches('/');
    let value = value.strip_suffix(".git").unwrap_or(value);
    let path = ["https://github.com/", "http://github.com/", "github.com/"]
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .unwrap_or(value);
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    (parts.next().is_none() && valid(owner) && valid(name) && !owner.eq_ignore_ascii_case("OWNER"))
        .then(|| format!("{owner}/{name}"))
}

/// The repository named at build time by `SHL_UPDATE_REPO` or, failing that,
/// by `repository` in Cargo.toml.
fn repo() -> Option<String> {
    let configured = option_env!("SHL_UPDATE_REPO").filter(|s| !s.trim().is_empty());
    parse_repo(configured.unwrap_or(env!("CARGO_PKG_REPOSITORY")))
}

fn source() -> Option<Source> {
    // Lets a debug build test updates against a local server.
    #[cfg(debug_assertions)]
    if let Ok(manifest_url) = std::env::var("SHL_UPDATE_MANIFEST_URL") {
        let page_url = repo().map_or_else(|| manifest_url.clone(), |repo| release_page(&repo));
        return Some(Source { manifest_url, page_url });
    }
    let repo = repo()?;
    Some(Source {
        manifest_url: format!("https://github.com/{repo}/releases/latest/download/latest.json"),
        page_url: release_page(&repo),
    })
}

fn release_page(repo: &str) -> String {
    format!("https://github.com/{repo}/releases/latest")
}

/// Whether this build knows where to look for updates.
pub fn is_configured() -> bool {
    source().is_some()
}

/// Key of this platform's archive in the manifest, such as `macos-aarch64`.
fn platform_key() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

// -- versions and manifest -----------------------------------------------------

/// `major.minor.patch` and the pre-release part of a version such as
/// `v1.2.3-beta.1+build`.
fn parse_version(version: &str) -> Option<([u64; 3], Option<&str>)> {
    let version = version.trim();
    let version = version.strip_prefix('v').unwrap_or(version);
    let version = version.split('+').next()?;
    let (core, pre) = match version.split_once('-') {
        Some((core, pre)) if !pre.is_empty() => (core, Some(pre)),
        Some(_) => return None,
        None => (version, None),
    };
    let mut numbers = core.split('.').map(|n| n.parse::<u64>().ok());
    let parsed = [numbers.next()??, numbers.next()??, numbers.next()??];
    numbers.next().is_none().then_some((parsed, pre))
}

/// Semver precedence of two pre-release parts.
fn compare_pre(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        let ordering = match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            },
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

/// Whether `candidate` is a later version than `current`; unparsable
/// versions never are.
fn is_newer(candidate: &str, current: &str) -> bool {
    use std::cmp::Ordering;
    let (Some((a, a_pre)), Some((b, b_pre))) = (parse_version(candidate), parse_version(current)) else {
        return false;
    };
    match a.cmp(&b) {
        Ordering::Equal => match (a_pre, b_pre) {
            (None, Some(_)) => true,
            (Some(x), Some(y)) => compare_pre(x, y) == Ordering::Greater,
            _ => false,
        },
        ordering => ordering == Ordering::Greater,
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    assets: HashMap<String, Asset>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct Asset {
    url: String,
    sha256: String,
    size: u64,
}

/// A release newer than the running app.
#[derive(Clone, Debug)]
struct Release {
    version: String,
    notes: String,
    /// The archive for this platform, `None` when the release has none.
    asset: Option<Asset>,
}

fn parse_manifest(text: &str) -> Result<Manifest> {
    let manifest: Manifest = serde_json::from_str(text).localize("err_update_manifest")?;
    if parse_version(&manifest.version).is_none() {
        return Err(LocalizedError::new("err_update_manifest").detail(&manifest.version).into());
    }
    Ok(manifest)
}

/// The usable archive for `key`: an http(s) URL and a well-formed checksum.
fn select_asset(manifest: &Manifest, key: &str) -> Option<Asset> {
    let asset = manifest.assets.get(key)?;
    let checksum_ok = asset.sha256.len() == 64 && asset.sha256.chars().all(|c| c.is_ascii_hexdigit());
    let url_ok = asset.url.starts_with("https://") || asset.url.starts_with("http://");
    (checksum_ok && url_ok && asset.size > 0).then(|| asset.clone())
}

/// The release in `manifest` when it is newer than `current`.
fn evaluate(manifest: Manifest, current: &str, key: &str) -> Option<Release> {
    if !is_newer(&manifest.version, current) {
        return None;
    }
    Some(Release {
        asset: select_asset(&manifest, key),
        version: manifest.version.trim().trim_start_matches('v').to_string(),
        notes: truncate_notes(manifest.notes.trim(), MAX_NOTES_CHARS),
    })
}

fn truncate_notes(notes: &str, max_chars: usize) -> String {
    match notes.char_indices().nth(max_chars) {
        Some((end, _)) => format!("{}…", notes[..end].trim_end()),
        None => notes.to_string(),
    }
}

fn due(last_check: Option<i64>, now: i64) -> bool {
    last_check.is_none_or(|last| now - last >= CHECK_INTERVAL_SECS || last > now)
}

// -- network -------------------------------------------------------------------

const USER_AGENT: &str = concat!("SwitchHostsLite/", env!("CARGO_PKG_VERSION"));

/// HTTP(S) GET through the curl that ships with macOS and Windows 10+, which
/// keeps a TLS stack out of the binary.
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod http {
    use std::io::{self, Read};
    use std::process::{Child, ChildStdout, Command, Stdio};
    use std::time::Duration;

    /// A response body streamed from curl's stdout.
    pub struct Body {
        child: Child,
        stdout: ChildStdout,
        received: u64,
        max_len: u64,
    }

    #[cfg(target_os = "macos")]
    fn curl() -> Command {
        Command::new("/usr/bin/curl")
    }

    #[cfg(target_os = "windows")]
    fn curl() -> Command {
        let system = std::env::var_os("SystemRoot")
            .map(|root| std::path::PathBuf::from(root).join("System32").join("curl.exe"))
            .filter(|curl| curl.is_file());
        super::hidden_command(system.as_deref().unwrap_or("curl.exe".as_ref()))
    }

    /// Start a GET of `url` that fails on an HTTP error status, after
    /// `timeout` or when the body is longer than `max_len`.
    pub fn get(url: &str, timeout: Duration, max_len: u64) -> io::Result<Body> {
        let mut child = curl()
            // Must come first: ignores the user's .curlrc.
            .arg("--disable")
            .args(["--silent", "--show-error", "--fail", "--location", "--max-redirs", "10"])
            .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
            .arg("--connect-timeout")
            .arg(super::CONNECT_TIMEOUT.as_secs().to_string())
            .arg("--max-time")
            .arg(timeout.as_secs().to_string())
            .arg("--max-filesize")
            .arg(max_len.to_string())
            .arg("--user-agent")
            .arg(super::USER_AGENT)
            .arg("--url")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| io::Error::new(err.kind(), format!("curl: {err}")))?;
        let stdout = child.stdout.take().expect("piped stdout");
        Ok(Body { child, stdout, received: 0, max_len })
    }

    impl Read for Body {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.stdout.read(buf)?;
            self.received += n as u64;
            if self.received > self.max_len {
                return Err(io::Error::other(format!("response longer than {} bytes", self.max_len)));
            }
            Ok(n)
        }
    }

    impl Body {
        /// Once the body was read to the end: curl's error if the transfer failed.
        pub fn finish(mut self) -> io::Result<()> {
            let mut stderr = String::new();
            if let Some(mut pipe) = self.child.stderr.take() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            let status = self.child.wait()?;
            if status.success() {
                return Ok(());
            }
            Err(io::Error::other(format!("curl: {status} {}", stderr.trim())))
        }
    }

    impl Drop for Body {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn fetch_manifest(url: &str) -> Result<Manifest> {
    let mut body = http::get(url, MANIFEST_TIMEOUT, MAX_MANIFEST_BYTES).localize("err_network")?;
    let mut text = String::new();
    body.read_to_string(&mut text).localize("err_network")?;
    body.finish().localize("err_network")?;
    parse_manifest(&text)
}

/// The download was cancelled by the user; not shown as an error.
#[derive(Debug)]
struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Check a downloaded archive against the manifest.
fn verify(size: u64, sha256: &str, asset: &Asset) -> Result<()> {
    if size != asset.size {
        let detail = format!("size {size}, expected {}", asset.size);
        return Err(LocalizedError::new("err_update_checksum").detail(detail).into());
    }
    if !sha256.eq_ignore_ascii_case(&asset.sha256) {
        let detail = format!("sha256 {sha256}, expected {}", asset.sha256);
        return Err(LocalizedError::new("err_update_checksum").detail(detail).into());
    }
    Ok(())
}

/// Download `asset` to `dest`, counting bytes in `progress`, then verify it.
fn download(asset: &Asset, dest: &Path, progress: &AtomicU64, cancel: &AtomicBool) -> Result<()> {
    let mut body = http::get(&asset.url, DOWNLOAD_TIMEOUT, asset.size).localize("err_update_download")?;
    let mut file = fs::File::create(dest).localize("err_update_download")?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 64 * 1024];
    let mut size = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = fs::remove_file(dest);
            return Err(Cancelled.into());
        }
        let n = body.read(&mut buf).localize("err_update_download")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).localize("err_update_download")?;
        size += n as u64;
        progress.store(size, Ordering::Relaxed);
    }
    body.finish().localize("err_update_download")?;
    file.sync_all().localize("err_update_download")?;
    verify(size, &hex(&hasher.finalize()), asset)
}

/// File name of a download URL, ignoring any query.
fn url_file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or_default();
    let name = path.rsplit('/').next().unwrap_or_default();
    let safe = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if safe { name.to_string() } else { "update-package".to_string() }
}

// -- installing ----------------------------------------------------------------

fn run(command: &mut Command, key: &'static str) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|err| LocalizedError::new(key).detail(format!("{program}: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = format!("{program}: {} {}", output.status, stderr.trim());
        return Err(LocalizedError::new(key).detail(detail).into());
    }
    Ok(())
}

/// Whether files can be created in `dir`.
fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".switch-hosts-lite-write-test-{}", std::process::id()));
    let writable = fs::File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    writable
}

/// The first entry in `dir`, or one level below it, matching `pred`.
fn find_entry(dir: &Path, pred: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    if let Some(found) = entries.iter().find(|p| pred(p)) {
        return Some(found.clone());
    }
    entries
        .iter()
        .filter(|p| p.is_dir())
        .find_map(|sub| fs::read_dir(sub).ok()?.flatten().map(|e| e.path()).find(|p| pred(p)))
}

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Put `new` in place of the binary `exe`, which may be running. On Windows
/// the running exe cannot be overwritten but can be renamed out of the way.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn replace_binary(new: &Path, exe: &Path) -> Result<()> {
    let staged = with_suffix(exe, ".new");
    let _ = fs::remove_file(&staged);
    fs::copy(new, &staged).localize("err_update_install")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).localize("err_update_install")?;
    }
    #[cfg(windows)]
    {
        let old = with_suffix(exe, ".old");
        let _ = fs::remove_file(&old);
        if let Err(err) = fs::rename(exe, &old) {
            let _ = fs::remove_file(&staged);
            return Err(err).localize("err_update_install");
        }
        if let Err(err) = fs::rename(&staged, exe) {
            let _ = fs::rename(&old, exe);
            let _ = fs::remove_file(&staged);
            return Err(err).localize("err_update_install");
        }
    }
    #[cfg(not(windows))]
    if let Err(err) = fs::rename(&staged, exe) {
        let _ = fs::remove_file(&staged);
        return Err(err).localize("err_update_install");
    }
    Ok(())
}

/// Remove what a previous update left next to the binary.
fn remove_leftovers() {
    if let Ok(exe) = std::env::current_exe() {
        for suffix in [".old", ".new"] {
            let _ = fs::remove_file(with_suffix(&exe, suffix));
        }
    }
}

/// The `.app` bundle of an executable at `X.app/Contents/MacOS/<bin>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let is_app = bundle.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("app"));
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app).then(|| bundle.to_path_buf())
}

/// A `/bin/sh` command that waits for this process to exit, then runs
/// `script` with the given path as `$0`.
#[cfg(target_os = "macos")]
fn relaunch_command(script: &str, path: &Path) -> Command {
    use std::os::unix::process::CommandExt as _;
    let wait = "i=0; while kill -0 \"$1\" 2>/dev/null && [ $i -lt 100 ]; do sleep 0.1; i=$((i+1)); done";
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(format!("{wait}; {script}"))
        .arg(path)
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    command
}

#[cfg(target_os = "macos")]
fn plist_version(plist: &Path) -> Option<String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(plist)
        .output()
        .ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// `'…'`-quote for `/bin/sh`.
#[cfg(target_os = "macos")]
fn sh_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// Unpack the archive and put its app in place of `bundle`.
#[cfg(target_os = "macos")]
fn install_macos(archive: &Path, work: &Path, bundle: &Path, version: &str) -> Result<()> {
    let staging = work.join("extract");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).localize("err_update_extract")?;
    run(
        Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(archive).arg(&staging),
        "err_update_extract",
    )?;
    let is_app = |p: &Path| p.is_dir() && p.extension().is_some_and(|ext| ext == "app");
    let new_app = find_entry(&staging, &is_app).ok_or_else(|| LocalizedError::new("err_update_invalid_package"))?;
    let found = plist_version(&new_app.join("Contents/Info.plist"));
    if found.as_deref() != Some(version) || !new_app.join("Contents/MacOS").is_dir() {
        let detail = format!("version {}, expected {version}", found.as_deref().unwrap_or("?"));
        return Err(LocalizedError::new("err_update_invalid_package").detail(detail).into());
    }
    let _ = Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&new_app)
        .output();
    replace_bundle(&new_app, bundle)
}

#[cfg(target_os = "macos")]
fn replace_bundle(new_app: &Path, bundle: &Path) -> Result<()> {
    let parent = bundle.parent().ok_or_else(|| LocalizedError::new("err_update_install"))?;
    let backup = with_suffix(bundle, ".update-backup");
    if is_writable(parent) {
        let _ = fs::remove_dir_all(&backup);
        fs::rename(bundle, &backup).localize("err_update_install")?;
        let moved = fs::rename(new_app, bundle)
            .or_else(|_| run(Command::new("/usr/bin/ditto").arg(new_app).arg(bundle), "err_update_install").map_err(std::io::Error::other));
        if let Err(err) = moved {
            let _ = fs::remove_dir_all(bundle);
            let _ = fs::rename(&backup, bundle);
            return Err(err).localize("err_update_install");
        }
        let _ = fs::remove_dir_all(&backup);
        return Ok(());
    }

    // Such as /Applications for a standard user: ask for an admin password.
    let (cur, bak, new) = (sh_quote(bundle), sh_quote(&backup), sh_quote(new_app));
    let shell = format!(
        "rm -rf {bak} && mv {cur} {bak} && if /usr/bin/ditto {new} {cur}; then rm -rf {bak}; \
         else rm -rf {cur}; mv {bak} {cur}; exit 1; fi"
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|err| LocalizedError::new("err_launch").arg("program", "osascript").detail(err))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // -128 is userCanceledErr, the password prompt was dismissed.
        let key = if stderr.contains("(-128)") { "err_auth_cancelled" } else { "err_update_install" };
        return Err(LocalizedError::new(key).detail(stderr.trim()).into());
    }
    Ok(())
}

/// Install the downloaded archive over the running app and return the
/// command that starts the new version.
#[cfg(target_os = "macos")]
fn install(archive: &Path, work: &Path, version: &str) -> Result<Command> {
    let exe = std::env::current_exe().localize("err_update_install")?;
    let bundle = bundle_root(&exe).ok_or_else(|| LocalizedError::new("err_update_dev_build"))?;
    // Gatekeeper runs quarantined apps from a read-only copy.
    if bundle.to_string_lossy().contains("/AppTranslocation/") {
        return Err(LocalizedError::new("err_update_translocated").into());
    }
    install_macos(archive, work, &bundle, version)?;
    let _ = fs::remove_dir_all(work);
    Ok(relaunch_command("open \"$0\"", &bundle))
}

#[cfg(target_os = "windows")]
fn install(archive: &Path, work: &Path, _version: &str) -> Result<Command> {
    let exe = std::env::current_exe().localize("err_update_install")?;
    if archive.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")) {
        return install_from_setup(archive, work, &exe);
    }
    let dir = exe.parent().ok_or_else(|| LocalizedError::new("err_update_install"))?;
    if !is_writable(dir) {
        return Err(LocalizedError::new("err_update_not_writable").arg("dir", dir.display()).into());
    }
    let staging = work.join("extract");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).localize("err_update_extract")?;
    extract(archive, &staging)?;
    let is_binary = |p: &Path| p.is_file() && p.file_name().is_some_and(|n| n == PACKAGE_BINARY);
    let new = find_entry(&staging, &is_binary).ok_or_else(|| LocalizedError::new("err_update_invalid_package"))?;
    replace_binary(&new, &exe)?;
    let _ = fs::remove_dir_all(work);
    Ok(start_command(&exe))
}

/// Argument that marks the detached helper started by `install_from_setup`:
/// `--install-update <setup> <exe> <pid>`.
#[cfg(any(windows, test))]
const UPDATE_INSTALL_ARG: &str = "--install-update";

/// The command line of the update helper, besides the program itself.
#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum InstallerArgs {
    /// An ordinary launch.
    Normal,
    /// The helper was started with a malformed command line.
    Invalid,
    /// Install `setup` once process `pid` has exited, then start `exe`.
    Install { setup: PathBuf, exe: PathBuf, pid: u32 },
}

/// Only exactly the flag and its three values count, so an ordinary launch
/// never installs an update.
#[cfg(any(windows, test))]
fn installer_args<I, S>(args: I) -> InstallerArgs
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    if args
        .get(1)
        .is_none_or(|arg| arg.as_os_str() != std::ffi::OsStr::new(UPDATE_INSTALL_ARG))
    {
        return InstallerArgs::Normal;
    }
    match args.as_slice() {
        [_, _, setup, exe, pid] => match pid.to_str().and_then(|pid| pid.parse().ok()) {
            Some(pid) => InstallerArgs::Install {
                setup: PathBuf::from(setup),
                exe: PathBuf::from(exe),
                pid,
            },
            None => InstallerArgs::Invalid,
        },
        _ => InstallerArgs::Invalid,
    }
}

/// A copy of this executable that installs `setup` after the app exits. The
/// installed exe cannot be overwritten while it runs, so the helper must not be
/// the installed image; it waits for `pid` to exit first.
#[cfg(target_os = "windows")]
fn install_from_setup(setup: &Path, work: &Path, exe: &Path) -> Result<Command> {
    let helper = work.join("switch-hosts-lite-update-helper.exe");
    let _ = fs::remove_file(&helper);
    fs::copy(exe, &helper).localize("err_update_install")?;
    let mut command = start_command(&helper);
    command
        .arg(UPDATE_INSTALL_ARG)
        .arg(setup)
        .arg(exe)
        .arg(std::process::id().to_string());
    Ok(command)
}

/// When this process is the update helper started by `install_from_setup`, wait
/// for the app to exit, run the installer elevated, and start the app again. An
/// ordinary launch returns immediately.
#[cfg(target_os = "windows")]
pub fn run_update_installer_helper() {
    match installer_args(std::env::args_os()) {
        InstallerArgs::Normal => {}
        InstallerArgs::Invalid => {
            eprintln!("update helper: expected {UPDATE_INSTALL_ARG} <setup> <exe> <pid>");
            std::process::exit(1);
        }
        InstallerArgs::Install { setup, exe, pid } => {
            wait_for_exit(pid);
            let installed = crate::hosts::run_elevated_process(
                &setup,
                std::ffi::OsStr::new("/S"),
                "err_update_install",
            );
            if let Err(err) = &installed {
                eprintln!("update helper: {err:#}");
            }
            // The installer is a single file; its work directory holds only the
            // running helper now, so it is removed on a later startup.
            let _ = fs::remove_file(&setup);
            if let Err(err) = start_command(&exe).spawn() {
                eprintln!("update helper: restart failed: {err}");
            }
            std::process::exit(if installed.is_ok() { 0 } else { 1 });
        }
    }
}

/// Wait until `pid` exits. A process that is already gone, or that cannot be
/// opened, is treated as exited.
#[cfg(target_os = "windows")]
fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if !process.is_null() {
            WaitForSingleObject(process, INFINITE);
            CloseHandle(process);
        }
    }
}

#[cfg(target_os = "windows")]
fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

/// Unzip with the bsdtar that ships with Windows 10+, else PowerShell.
#[cfg(target_os = "windows")]
fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let tar = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("tar.exe"))
        .filter(|tar| tar.is_file());
    if let Some(tar) = tar
        && run(hidden_command(tar).arg("-xf").arg(archive).arg("-C").arg(dest), "err_update_extract").is_ok()
    {
        return Ok(());
    }
    let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
    let script = format!(
        "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
        quote(archive),
        quote(dest)
    );
    run(
        hidden_command("powershell").args(["-NoProfile", "-NonInteractive", "-Command", script.as_str()]),
        "err_update_extract",
    )
}

#[cfg(target_os = "windows")]
fn start_command(exe: &Path) -> Command {
    use std::os::windows::process::CommandExt as _;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let mut command = Command::new(exe);
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    command
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn install(_: &Path, _: &Path, _: &str) -> Result<Command> {
    Err(LocalizedError::new("err_update_install").detail(std::env::consts::OS).into())
}

/// A fresh scratch directory for one update.
fn work_dir(version: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("switch-hosts-lite-update-{version}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).localize("err_update_download")?;
    Ok(dir)
}

/// Remove scratch directories left by earlier updates. The Windows helper
/// cannot delete its own copy while it runs, so its directory stays behind.
fn remove_stale_update_dirs() {
    let Ok(entries) = std::env::temp_dir().read_dir() else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with("switch-hosts-lite-update-") {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

// -- UI ------------------------------------------------------------------------

#[derive(Default)]
struct UpdateState {
    checking: bool,
    job: Option<Job>,
}

impl Global for UpdateState {}

struct Job {
    downloaded: Arc<AtomicU64>,
    total: u64,
    cancel: Arc<AtomicBool>,
    phase: Phase,
}

enum Phase {
    Downloading,
    Installing,
    Failed(String),
}

impl UpdateState {
    fn busy(&self) -> bool {
        matches!(&self.job, Some(job) if !matches!(job.phase, Phase::Failed(_)))
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Run `f` with the active window, or any window.
fn with_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App)) {
    let Some(handle) = cx.active_window().or_else(|| cx.windows().into_iter().next()) else {
        return;
    };
    handle.update(cx, |_, window, cx| f(window, cx)).ok();
}

fn notify(note: Notification, cx: &mut App) {
    with_window(cx, |window, cx| window.push_notification(note.id::<UpdateState>(), cx));
}

/// Clean up after a previous update and schedule the automatic check.
pub fn init(cx: &mut App) {
    cx.set_global(UpdateState::default());
    cx.spawn(async move |cx| {
        cx.background_executor().timer(STARTUP_DELAY).await;
        cx.background_executor().spawn(async {
            remove_leftovers();
            remove_stale_update_dirs();
        })
        .await;
        cx.update(|cx| {
            let settings = Settings::global(cx);
            if settings.check_updates && due(settings.last_update_check, now()) && is_configured() {
                check(false, cx);
            }
        });
    })
    .detach();
}

/// Look for a newer release. A manual check reports every outcome; an
/// automatic one only a found update.
pub fn check(manual: bool, cx: &mut App) {
    let Some(source) = source() else {
        if manual {
            notify(Notification::warning(t!("update_not_configured")), cx);
        }
        return;
    };
    let state = cx.default_global::<UpdateState>();
    if state.checking || state.busy() {
        return;
    }
    state.checking = true;
    Settings::update(cx, |s| s.last_update_check = Some(now()));
    if manual {
        notify(Notification::info(t!("update_checking")), cx);
    }

    let url = source.manifest_url.clone();
    cx.spawn(async move |cx| {
        let result = cx.background_executor().spawn(async move { fetch_manifest(&url) }).await;
        cx.update(|cx| {
            cx.default_global::<UpdateState>().checking = false;
            match result.map(|manifest| evaluate(manifest, CURRENT_VERSION, &platform_key())) {
                Ok(Some(release)) if manual => open_dialog(Rc::new(release), source, cx),
                Ok(Some(release)) => notify_available(Rc::new(release), source, cx),
                Ok(None) if manual => {
                    notify(Notification::success(t!("up_to_date", version = CURRENT_VERSION)), cx);
                }
                Ok(None) => {}
                Err(err) if manual => {
                    notify(Notification::error(t!("update_check_failed", error = error_text(&err))), cx);
                }
                Err(err) => eprintln!("update check failed: {err:#}"),
            }
        });
    })
    .detach();
}

fn notify_available(release: Rc<Release>, source: Source, cx: &mut App) {
    let note = Notification::info(t!("update_available", version = release.version)).action(move |_, _, cx| {
        let (release, source, note) = (release.clone(), source.clone(), cx.entity());
        Button::new("update-view")
            .primary()
            .label(t!("update_view"))
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                note.update(cx, |note, cx| note.dismiss(window, cx));
                open_dialog(release.clone(), source.clone(), cx);
            })
    });
    notify(note, cx);
}

/// Stop a running download, or forget a failed update.
fn dismiss_job(cx: &mut App) {
    let state = cx.default_global::<UpdateState>();
    match &state.job {
        Some(job) if matches!(job.phase, Phase::Downloading) => job.cancel.store(true, Ordering::Relaxed),
        Some(job) if matches!(job.phase, Phase::Failed(_)) => state.job = None,
        _ => {}
    }
}

fn format_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.)
}

fn open_dialog(release: Rc<Release>, source: Source, cx: &mut App) {
    if cx.default_global::<UpdateState>().job.as_ref().is_some_and(|job| matches!(job.phase, Phase::Failed(_))) {
        cx.global_mut::<UpdateState>().job = None;
    }
    with_window(cx, move |window, cx| {
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (muted, muted_bg, border, danger) = (theme.muted_foreground, theme.muted, theme.border, theme.danger);
            let state = cx.global::<UpdateState>();
            let busy = state.busy();
            let status = match &state.job {
                Some(job) => match &job.phase {
                    Phase::Downloading => {
                        let done = job.downloaded.load(Ordering::Relaxed);
                        let percent = done as f32 * 100. / job.total.max(1) as f32;
                        let label = t!("update_downloading", done = format_mb(done), total = format_mb(job.total));
                        v_flex()
                            .gap_1()
                            .child(Progress::new("update-progress").value(percent))
                            .child(div().text_xs().text_color(muted).child(label))
                    }
                    Phase::Installing => v_flex()
                        .gap_1()
                        .child(Progress::new("update-progress").loading(true))
                        .child(div().text_xs().text_color(muted).child(t!("update_installing"))),
                    Phase::Failed(message) => v_flex()
                        .text_sm()
                        .text_color(danger)
                        .child(t!("update_failed", error = message)),
                },
                None if release.asset.is_none() => v_flex()
                    .text_sm()
                    .text_color(muted)
                    .child(t!("err_update_no_asset")),
                None => v_flex(),
            };
            let notes = if release.notes.is_empty() {
                t!("update_no_notes").to_string()
            } else {
                release.notes.clone()
            };

            let page_url = source.page_url.clone();
            let update_release = release.clone();
            let footer = h_flex()
                .w_full()
                .justify_between()
                .child(
                    Button::new("update-page")
                        .link()
                        .label(t!("open_download_page"))
                        .on_click(move |_, _, cx| cx.open_url(&page_url)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Button::new("update-later").label(t!("update_later")).cursor_pointer().on_click(
                            |_, window, cx| {
                                dismiss_job(cx);
                                window.close_dialog(cx);
                            },
                        ))
                        .child(
                            Button::new("update-now")
                                .primary()
                                .label(t!("update_now"))
                                .loading(busy)
                                .disabled(busy || release.asset.is_none())
                                .when(!busy && release.asset.is_some(), |this| this.cursor_pointer())
                                .on_click(move |_, _, cx| start_update(update_release.clone(), cx)),
                        ),
                );

            dialog
                .title(t!("update_available", version = release.version))
                .w(px(480.))
                .on_cancel(|_, _, cx| {
                    dismiss_job(cx);
                    true
                })
                .child(
                    v_flex()
                        .gap_3()
                        .child(div().text_sm().text_color(muted).child(t!(
                            "update_current_version",
                            version = CURRENT_VERSION
                        )))
                        .child(
                            div()
                                .id("update-notes")
                                .max_h(px(240.))
                                .p_2()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(muted_bg)
                                .text_sm()
                                .overflow_y_scrollbar()
                                .child(notes),
                        )
                        .child(status),
                )
                .footer(footer)
        });
    });
}

fn set_phase(phase: Phase, cx: &mut App) {
    if let Some(job) = &mut cx.default_global::<UpdateState>().job {
        job.phase = phase;
    }
    cx.refresh_windows();
}

fn fail(err: anyhow::Error, cx: &mut App) {
    if err.is::<Cancelled>() {
        cx.default_global::<UpdateState>().job = None;
        cx.refresh_windows();
        return;
    }
    let message = error_text(&err);
    set_phase(Phase::Failed(message), cx);
}

fn start_update(release: Rc<Release>, cx: &mut App) {
    let Some(asset) = release.asset.clone() else {
        return;
    };
    let state = cx.default_global::<UpdateState>();
    if state.busy() {
        return;
    }
    let downloaded = Arc::new(AtomicU64::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    state.job = Some(Job {
        downloaded: downloaded.clone(),
        total: asset.size,
        cancel: cancel.clone(),
        phase: Phase::Downloading,
    });
    cx.refresh_windows();

    let finished = Arc::new(AtomicBool::new(false));
    let poll_finished = finished.clone();
    cx.spawn(async move |cx| {
        while !poll_finished.load(Ordering::Relaxed) {
            cx.background_executor().timer(PROGRESS_INTERVAL).await;
            cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();

    let version = release.version.clone();
    cx.spawn(async move |cx| {
        let downloaded_archive = cx
            .background_executor()
            .spawn(async move {
                let work = work_dir(&version)?;
                let archive = work.join(url_file_name(&asset.url));
                download(&asset, &archive, &downloaded, &cancel)?;
                anyhow::Ok((work, archive, version))
            })
            .await;
        let (work, archive, version) = match downloaded_archive {
            Ok(downloaded) => downloaded,
            Err(err) => {
                finished.store(true, Ordering::Relaxed);
                cx.update(|cx| fail(err, cx));
                return;
            }
        };
        cx.update(|cx| set_phase(Phase::Installing, cx));
        let installed = cx
            .background_executor()
            .spawn(async move {
                let relaunch = install(&archive, &work, &version);
                // A successful install removes its own work directory; the
                // Windows helper needs it to hold the installer until it runs.
                if relaunch.is_err() {
                    let _ = fs::remove_dir_all(&work);
                }
                relaunch
            })
            .await;
        finished.store(true, Ordering::Relaxed);
        cx.update(|cx| match installed {
            Ok(relaunch) => restart(relaunch, cx),
            Err(err) => fail(err, cx),
        });
    })
    .detach();
}

/// Start the installed version and quit this one through the app's own quit
/// path, which writes out pending state.
fn restart(mut relaunch: Command, cx: &mut App) {
    if let Err(err) = relaunch.spawn() {
        let err = LocalizedError::new("err_update_relaunch").detail(err);
        set_phase(Phase::Failed(err.message()), cx);
        return;
    }
    if let Err(err) = Settings::global(cx).save() {
        eprintln!("save settings failed: {err:#}");
    }
    cx.dispatch_action(&crate::Quit);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("1.0.0", "1.0.0-beta.2"));
        assert!(is_newer("1.0.0-beta.11", "1.0.0-beta.2"));
        assert!(is_newer("1.0.0-beta", "1.0.0-alpha.1"));
        assert!(is_newer("1.0.0-alpha.1", "1.0.0-alpha"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("1.0.0+build.5", "1.0.0"));
        assert!(!is_newer("1.0.0-rc.1", "1.0.0"));
        assert!(!is_newer("0.9.9", "1.0.0"));
        assert!(!is_newer("1.0", "0.1.0"));
        assert!(!is_newer("1.0.0.1", "0.1.0"));
        assert!(!is_newer("1.0.0-", "0.1.0"));
        assert!(!is_newer("latest", "0.1.0"));
        assert_eq!(parse_version("v1.2.3-rc.1+x"), Some(([1, 2, 3], Some("rc.1"))));
    }

    #[test]
    fn platform_key_matches_release_assets() {
        let key = platform_key();
        let expected = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "macos-aarch64"
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            "macos-x86_64"
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            "windows-x86_64"
        } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
            "windows-aarch64"
        } else {
            return;
        };
        assert_eq!(key, expected);
    }

    const MANIFEST: &str = r#"{
        "version": "v0.3.0",
        "notes": "Fixes\n- a bug",
        "pub_date": "2026-11-06T00:00:00Z",
        "assets": {
            "macos-aarch64": {"url": "https://example.com/a.zip", "sha256": "AB0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd", "size": 10},
            "freebsd-x86_64": {"url": "ftp://example.com/a.zip", "sha256": "ab0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcd", "size": 10},
            "windows-x86_64": {"url": "https://example.com/a.zip", "sha256": "not-hex", "size": 10}
        }
    }"#;

    #[test]
    fn parses_manifest_and_selects_assets() {
        let manifest = parse_manifest(MANIFEST).unwrap();
        assert_eq!(manifest.version, "v0.3.0");
        let asset = select_asset(&manifest, "macos-aarch64").unwrap();
        assert_eq!((asset.url.as_str(), asset.size), ("https://example.com/a.zip", 10));
        assert_eq!(select_asset(&manifest, "freebsd-x86_64"), None);
        assert_eq!(select_asset(&manifest, "windows-x86_64"), None);
        assert_eq!(select_asset(&manifest, "windows-aarch64"), None);

        let release = evaluate(parse_manifest(MANIFEST).unwrap(), "0.2.9", "macos-aarch64").unwrap();
        assert_eq!((release.version.as_str(), release.notes.as_str()), ("0.3.0", "Fixes\n- a bug"));
        assert!(release.asset.is_some());
        assert!(evaluate(parse_manifest(MANIFEST).unwrap(), "0.2.9", "freebsd-aarch64").unwrap().asset.is_none());
        assert!(evaluate(parse_manifest(MANIFEST).unwrap(), "0.3.0", "macos-aarch64").is_none());

        assert!(parse_manifest(r#"{"version": "next"}"#).is_err());
        assert!(parse_manifest("<html>").is_err());
        let minimal = parse_manifest(r#"{"version": "1.0.0"}"#).unwrap();
        assert!(minimal.assets.is_empty() && minimal.notes.is_empty());
    }

    #[test]
    fn verifies_size_and_checksum() {
        let digest = hex(&Sha256::digest(b"abc"));
        assert_eq!(digest, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let asset = Asset { url: String::new(), sha256: digest.to_uppercase(), size: 3 };
        assert!(verify(3, &digest, &asset).is_ok());
        assert!(verify(4, &digest, &asset).is_err());
        assert!(verify(3, &hex(&Sha256::digest(b"abd")), &asset).is_err());
    }

    #[test]
    fn finds_bundle_root() {
        let exe = Path::new("/Applications/SwitchHosts Lite.app/Contents/MacOS/switch-hosts-lite");
        assert_eq!(bundle_root(exe), Some(PathBuf::from("/Applications/SwitchHosts Lite.app")));
        assert_eq!(bundle_root(Path::new("/repo/target/debug/switch-hosts-lite")), None);
        assert_eq!(bundle_root(Path::new("/x/Foo/Contents/MacOS/bin")), None);
        assert_eq!(bundle_root(Path::new("/x/Foo.app/Contents/Resources/bin")), None);
    }

    #[test]
    fn detects_placeholder_repository() {
        assert_eq!(parse_repo("https://github.com/OWNER/SwitchHostsLite"), None);
        assert_eq!(parse_repo("https://github.com/jarven/SwitchHostsLite"), Some("jarven/SwitchHostsLite".into()));
        assert_eq!(parse_repo("https://github.com/a-b/c.d.git/"), Some("a-b/c.d".into()));
        assert_eq!(parse_repo("jarven/SwitchHostsLite"), Some("jarven/SwitchHostsLite".into()));
        assert_eq!(parse_repo("https://gitlab.com/a/b"), None);
        assert_eq!(parse_repo("https://github.com/a/b/tree/main"), None);
        assert_eq!(parse_repo(""), None);
        assert_eq!(repo().is_some(), !env!("CARGO_PKG_REPOSITORY").contains("/OWNER/") || option_env!("SHL_UPDATE_REPO").is_some());
    }

    #[test]
    fn parses_update_helper_arguments() {
        assert_eq!(installer_args(["app"]), InstallerArgs::Normal);
        assert_eq!(installer_args(["app", "--other", "x"]), InstallerArgs::Normal);
        let setup = r"C:\Temp\setup.exe";
        let exe = r"C:\Program Files\App\app.exe";
        assert_eq!(
            installer_args(["app", UPDATE_INSTALL_ARG, setup, exe, "42"]),
            InstallerArgs::Install { setup: PathBuf::from(setup), exe: PathBuf::from(exe), pid: 42 }
        );
        assert_eq!(installer_args(["app", UPDATE_INSTALL_ARG]), InstallerArgs::Invalid);
        assert_eq!(installer_args(["app", UPDATE_INSTALL_ARG, "setup", "exe"]), InstallerArgs::Invalid);
        assert_eq!(
            installer_args(["app", UPDATE_INSTALL_ARG, "setup", "exe", "nope"]),
            InstallerArgs::Invalid
        );
        assert_eq!(
            installer_args(["app", UPDATE_INSTALL_ARG, "setup", "exe", "1", "extra"]),
            InstallerArgs::Invalid
        );
    }

    #[test]
    fn small_helpers() {
        assert_eq!(truncate_notes("abcdef", 3), "abc…");
        assert_eq!(truncate_notes("说明文字", 10), "说明文字");
        assert_eq!(truncate_notes("说明文字", 2), "说明…");
        assert!(due(None, 100));
        assert!(!due(Some(100), 100 + CHECK_INTERVAL_SECS - 1));
        assert!(due(Some(100), 100 + CHECK_INTERVAL_SECS));
        assert!(due(Some(200), 100));
        assert_eq!(url_file_name("https://x/y/App-1.0-macos.zip?a=b"), "App-1.0-macos.zip");
        assert_eq!(url_file_name("https://x/y/../"), "update-package");
        assert_eq!(with_suffix(Path::new("/a/b.exe"), ".old"), PathBuf::from("/a/b.exe.old"));
    }

    /// Updates an app bundle from a local release, by hand:
    /// `SHL_TEST_MANIFEST_URL=http://127.0.0.1:8765/latest.json SHL_TEST_BUNDLE=/tmp/x/App.app
    /// cargo test simulate_update -- --ignored`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn simulate_update() {
        let url = std::env::var("SHL_TEST_MANIFEST_URL").unwrap();
        let bundle = PathBuf::from(std::env::var("SHL_TEST_BUNDLE").unwrap());
        let plist = bundle.join("Contents/Info.plist");
        let before = plist_version(&plist).unwrap();
        let release = evaluate(fetch_manifest(&url).unwrap(), &before, &platform_key()).expect("a newer release");
        let asset = release.asset.expect("an archive for this platform");

        let work = work_dir(&release.version).unwrap();
        let archive = work.join(url_file_name(&asset.url));
        let progress = AtomicU64::new(0);
        let cancelled = download(&asset, &archive, &progress, &AtomicBool::new(true)).unwrap_err();
        assert!(cancelled.is::<Cancelled>() && !archive.exists());
        download(&asset, &archive, &progress, &AtomicBool::new(false)).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), asset.size);

        let wrong = install_macos(&archive, &work, &bundle, "99.0.0").unwrap_err();
        eprintln!("mismatched package refused: {wrong:#}");
        assert_eq!(plist_version(&plist), Some(before));

        install_macos(&archive, &work, &bundle, &release.version).unwrap();
        assert_eq!(plist_version(&plist), Some(release.version.clone()));
        assert!(!with_suffix(&bundle, ".update-backup").exists());
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn replaces_binary_in_place() {
        let dir = std::env::temp_dir().join(format!("shl-updater-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (exe, new) = (dir.join("app"), dir.join("new-app"));
        fs::write(&exe, "old").unwrap();
        fs::write(&new, "new").unwrap();
        replace_binary(&new, &exe).unwrap();
        assert_eq!(fs::read_to_string(&exe).unwrap(), "new");
        assert!(!with_suffix(&exe, ".new").exists());
        assert!(is_writable(&dir));
        let found = find_entry(&dir, &|p: &Path| p.file_name().is_some_and(|n| n == "app"));
        assert_eq!(found, Some(exe));
        fs::remove_dir_all(&dir).unwrap();
    }
}
