//! Browser-scoped hosts: Chromium `--host-resolver-rules` profiles that never
//! touch the system hosts file.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

use crate::store::new_id;

/// Windows `CreateProcess` caps the command line at 32767 characters; leave
/// headroom. Unix `ARG_MAX` is far larger but still bounded.
pub fn cmd_limit() -> usize {
    if cfg!(windows) { 32_000 } else { 262_144 }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    #[default]
    // Older backups may still say "brave"; load those as Chrome rather than
    // failing the whole data file.
    #[serde(alias = "brave")]
    Chrome,
    Edge,
}

impl BrowserKind {
    pub const ALL: [Self; 2] = [Self::Chrome, Self::Edge];

    pub fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Google Chrome",
            Self::Edge => "Microsoft Edge",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Edge => "Edge",
        }
    }

    /// macOS app bundle and process name.
    #[cfg(target_os = "macos")]
    fn bundle(self) -> &'static str {
        match self {
            Self::Chrome => "Google Chrome",
            Self::Edge => "Microsoft Edge",
        }
    }

    fn exe_name(self) -> &'static str {
        match self {
            Self::Chrome => "chrome.exe",
            Self::Edge => "msedge.exe",
        }
    }

    /// Windows install layout below Program Files and LOCALAPPDATA.
    #[cfg(any(windows, test))]
    fn windows_dirs(self) -> (&'static str, &'static str) {
        match self {
            Self::Chrome => ("Google", "Chrome"),
            Self::Edge => ("Microsoft", "Edge"),
        }
    }

    /// Install locations probed in order, per platform.
    fn candidates(self) -> Vec<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            let mut roots = vec![PathBuf::from("/Applications")];
            if let Some(home) = dirs::home_dir() {
                roots.push(home.join("Applications"));
            }
            roots
                .into_iter()
                .map(|root| {
                    root.join(format!("{}.app", self.bundle()))
                        .join("Contents/MacOS")
                        .join(self.bundle())
                })
                .collect()
        }
        #[cfg(windows)]
        {
            windows_candidates(
                self,
                &std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into()),
                &std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into()),
                &std::env::var("LOCALAPPDATA").unwrap_or_default(),
            )
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Vec::new()
        }
    }

    /// The first candidate that exists, or `None` when the browser is missing.
    pub fn detected_path(self) -> Option<PathBuf> {
        self.candidates().into_iter().find(|path| path.exists())
    }

    fn default_path(self) -> PathBuf {
        self.candidates()
            .into_iter()
            .next()
            .unwrap_or_else(|| PathBuf::from(self.exe_name()))
    }

}

#[cfg(any(windows, test))]
fn windows_candidates(kind: BrowserKind, files: &str, files_x86: &str, local: &str) -> Vec<PathBuf> {
    let (company, product) = kind.windows_dirs();
    let mut roots = vec![PathBuf::from(files), PathBuf::from(files_x86)];
    if !local.is_empty() {
        roots.push(PathBuf::from(local));
    }
    roots
        .into_iter()
        .map(|root| {
            root.join(company)
                .join(product)
                .join("Application")
                .join(kind.exe_name())
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileMode {
    /// Own persistent `--user-data-dir`, logins survive restarts.
    #[default]
    Independent,
    /// Incognito in a temp dir the app deletes once the browser exits.
    Ephemeral,
    /// The browser's everyday profile; flags need a fully quit browser.
    Default,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RevKind {
    #[default]
    Mode,
    Browser,
    Name,
    Path,
    Color,
    Image,
    /// The hosts switches changed.
    Selection,
    /// Snapshot taken right before restoring an older revision.
    Restore,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProfileSnapshot {
    pub name: String,
    #[serde(default)]
    pub browser: BrowserKind,
    #[serde(default)]
    pub mode: ProfileMode,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom_image: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileRev {
    pub id: String,
    pub created_at: i64,
    #[serde(default)]
    pub kind: RevKind,
    pub snapshot: ProfileSnapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BrowserProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub browser: BrowserKind,
    #[serde(default)]
    pub mode: ProfileMode,
    /// Manual executable override for portable or custom installs.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Ids switched on in this profile, with the same folder and group rules
    /// as the hosts list but independent of the system switches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<String>,
    /// Folders collapsed in this profile's hosts tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed: Vec<String>,
    /// Personalization color of the launch marks (window tint, Dock tile,
    /// taskbar badge). `None` leaves the browser unmarked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_color: Option<[u8; 3]>,
    /// A user picture replaces the composited icon mark (macOS Dock only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub custom_image: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revs: Vec<ProfileRev>,
}

/// Preset mark colors, assigned round-robin to new profiles.
pub const TILE_PALETTE: [[u8; 3]; 8] = [
    [229, 72, 77],
    [242, 140, 40],
    [247, 197, 45],
    [70, 168, 96],
    [38, 166, 154],
    [52, 120, 246],
    [124, 92, 232],
    [214, 84, 168],
];

/// The stored mark is opaque sRGB bytes; the color picker works in Hsla.
pub fn color_to_hsla(color: [u8; 3]) -> gpui_kit::Hsla {
    let [r, g, b] = color.map(|c| f32::from(c) / 255.);
    gpui_kit::Rgba { r, g, b, a: 1. }.into()
}

pub fn hsla_to_color(value: gpui_kit::Hsla) -> [u8; 3] {
    let rgb = value.to_rgb();
    [rgb.r, rgb.g, rgb.b].map(|c| (c * 255.).round() as u8)
}

impl BrowserProfile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            browser: BrowserKind::default(),
            mode: ProfileMode::default(),
            path: String::new(),
            switches: Vec::new(),
            collapsed: Vec::new(),
            tile_color: None,
            custom_image: false,
            revs: Vec::new(),
        }
    }

    pub fn snapshot(&self) -> ProfileSnapshot {
        ProfileSnapshot {
            name: self.name.clone(),
            browser: self.browser,
            mode: self.mode,
            path: self.path.clone(),
            switches: self.switches.clone(),
            tile_color: self.tile_color,
            custom_image: self.custom_image,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: &ProfileSnapshot) {
        self.name = snapshot.name.clone();
        self.browser = snapshot.browser;
        self.mode = snapshot.mode;
        self.path = snapshot.path.clone();
        self.switches = snapshot.switches.clone();
        self.tile_color = snapshot.tile_color;
        self.custom_image = snapshot.custom_image;
    }

    pub fn push_rev(&mut self, kind: RevKind) {
        let snapshot = self.snapshot();
        self.push_rev_snapshot(kind, snapshot);
    }

    /// Record a state taken before a change, so restoring it is possible.
    pub fn push_rev_snapshot(&mut self, kind: RevKind, snapshot: ProfileSnapshot) {
        self.revs.insert(
            0,
            ProfileRev {
                id: new_id(),
                created_at: chrono::Local::now().timestamp(),
                kind,
                snapshot,
            },
        );
        self.revs.truncate(REV_LIMIT);
    }

    /// The executable to launch: the manual override, else auto detection.
    pub fn executable(&self) -> Option<PathBuf> {
        let manual = self.path.trim();
        if manual.is_empty() {
            self.browser.detected_path()
        } else {
            Some(PathBuf::from(manual))
        }
    }

    pub fn display_executable(&self) -> PathBuf {
        self.executable().unwrap_or_else(|| self.browser.default_path())
    }
}

/// The executable inside a macOS `.app` bundle: the file named like the
/// bundle, else the first file in `Contents/MacOS`. `None` without that dir.
pub fn app_executable(app: &Path) -> Option<PathBuf> {
    let stem = app.file_stem()?;
    let mut first = None;
    for entry in std::fs::read_dir(app.join("Contents/MacOS")).ok()? {
        let path = entry.ok()?.path();
        if !path.is_file() {
            continue;
        }
        if path.file_stem().is_some_and(|name| name == stem) {
            return Some(path);
        }
        first.get_or_insert(path);
    }
    first
}

const REV_LIMIT: usize = 20;

/// `(host, ip)` pairs of the enabled hosts content, in file order; the first
/// mapping of a host wins, like the resolver's own lookup. Comment tails and
/// lines whose first field is not an IP are skipped: Chromium's rules have no
/// comment syntax and would take the words for host names.
pub fn mappings(content: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();
    for line in content.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(ip) = fields.next() else {
            continue;
        };
        if ip.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        for host in fields {
            if seen.insert(host.to_string()) {
                out.push((host.to_string(), ip.to_string()));
            }
        }
    }
    out
}

/// Chromium parses a bare `::1` as `host:port`; IPv6 targets need brackets.
pub fn format_target(ip: &str) -> String {
    if ip.contains(':') { format!("[{ip}]") } else { ip.to_string() }
}

pub fn ephemeral_dir(profile: &BrowserProfile, tmp: &Path) -> PathBuf {
    tmp.join(format!("shl-ephemeral-{}", profile.id))
}

fn profile_dir(profile: &BrowserProfile, data_dir: &Path) -> PathBuf {
    data_dir.join("browser-profiles").join(&profile.id)
}

fn rules_value(maps: &[(String, String)]) -> String {
    maps.iter()
        .map(|(host, ip)| format!("MAP {host} {}", format_target(ip)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The mark color to apply at launch: personalization colors only mark own
/// instances, never the everyday browser.
pub fn mark_color(profile: &BrowserProfile) -> Option<[u8; 3]> {
    if profile.mode != ProfileMode::Default {
        profile.tile_color
    } else {
        None
    }
}

/// The launch command as a copyable string, mirroring `launch_args` minus the
/// fd-based debugging pipe, which only works when this app spawns the child.
pub fn build_cmd(
    profile: &BrowserProfile,
    maps: &[(String, String)],
    data_dir: &Path,
    tmp: &Path,
    mark: Option<[u8; 3]>,
) -> String {
    let mut cmd = format!("\"{}\"", profile.display_executable().display());
    match profile.mode {
        ProfileMode::Independent => {
            cmd += &format!(" --user-data-dir=\"{}\"", profile_dir(profile, data_dir).display());
        }
        ProfileMode::Ephemeral => {
            cmd += &format!(
                " --incognito --user-data-dir=\"{}\"",
                ephemeral_dir(profile, tmp).display()
            );
        }
        ProfileMode::Default => {}
    }
    // Chrome shows an "unsupported command-line flag" bar for
    // --host-resolver-rules; test type is the only switch that turns it off.
    cmd += " --test-type";
    cmd += &format!(" --host-resolver-rules=\"{}\"", rules_value(maps));
    if let Some(color) = mark {
        cmd += &format!(" --install-autogenerated-theme={}", theme_value(color));
    }
    cmd
}

pub fn launch_args(
    profile: &BrowserProfile,
    maps: &[(String, String)],
    data_dir: &Path,
    tmp: &Path,
    mark: Option<[u8; 3]>,
) -> Vec<String> {
    let mut args = Vec::new();
    if profile.mode == ProfileMode::Ephemeral {
        args.push("--incognito".into());
    }
    if let Some(arg) = user_data_arg(profile, data_dir, tmp) {
        args.push(arg);
    }
    args.push("--test-type".into());
    args.push(format!("--host-resolver-rules={}", rules_value(maps)));
    if let Some(color) = mark {
        args.push(format!("--install-autogenerated-theme={}", theme_value(color)));
    }
    args
}

fn theme_value(color: [u8; 3]) -> String {
    let [r, g, b] = color;
    format!("{r},{g},{b}")
}

/// The profile's own `--user-data-dir`, if it has one.
#[cfg(any(target_os = "macos", windows, test))]
pub fn user_data_dir(profile: &BrowserProfile, data_dir: &Path, tmp: &Path) -> Option<PathBuf> {
    match profile.mode {
        ProfileMode::Independent => Some(profile_dir(profile, data_dir)),
        ProfileMode::Ephemeral => Some(ephemeral_dir(profile, tmp)),
        ProfileMode::Default => None,
    }
}

/// The `--user-data-dir` argument that identifies this profile's own browser
/// instance, if it has one.
fn user_data_arg(profile: &BrowserProfile, data_dir: &Path, tmp: &Path) -> Option<String> {
    match profile.mode {
        ProfileMode::Independent => Some(format!("--user-data-dir={}", profile_dir(profile, data_dir).display())),
        ProfileMode::Ephemeral => Some(format!("--user-data-dir={}", ephemeral_dir(profile, tmp).display())),
        ProfileMode::Default => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    NoMappings,
    Missing,
    TooLong,
    Running,
}

/// Why launching is refused, in priority order. `running` says whether the
/// profile's browser is open: any instance for `Default`, this profile's own
/// instance for the other modes.
pub fn block_reason(profile: &BrowserProfile, maps: usize, cmd_len: usize, running: bool) -> Option<Block> {
    if maps == 0 {
        return Some(Block::NoMappings);
    }
    if profile.executable().is_none() {
        return Some(Block::Missing);
    }
    if cmd_len > cmd_limit() {
        return Some(Block::TooLong);
    }
    if running {
        return Some(Block::Running);
    }
    None
}

/// Whether the browser's everyday instance is open — the one with no
/// `--user-data-dir`, so instances started for other profiles never count.
pub fn default_running(kind: BrowserKind) -> bool {
    command_lines().iter().any(|line| is_default_main(line, kind))
}

/// Ids of profiles whose own browser instance is running. Instances started
/// before this app process (or by an earlier session) are found through the
/// profile's `--user-data-dir` in process command lines; Chromium keeps them
/// alive independently of the app.
pub fn running_profiles(profiles: &[BrowserProfile], data_dir: &Path, tmp: &Path) -> HashSet<String> {
    matching_profiles(&command_lines(), profiles, data_dir, tmp)
}

fn matching_profiles(
    lines: &[String],
    profiles: &[BrowserProfile],
    data_dir: &Path,
    tmp: &Path,
) -> HashSet<String> {
    profiles
        .iter()
        .filter(|profile| match user_data_arg(profile, data_dir, tmp) {
            Some(arg) => lines.iter().any(|line| line.contains(&arg)),
            // A `Default` profile has no directory of its own: it is the
            // everyday instance, the one started without the flag. Instances
            // this app starts for other profiles always pass one, so they can
            // never make it look open.
            None => {
                profile.mode == ProfileMode::Default
                    && lines.iter().any(|line| is_default_main(line, profile.browser))
            }
        })
        .map(|profile| profile.id.clone())
        .collect()
}

/// Whether a command line is the browser's everyday process: the executable
/// itself, without a `--user-data-dir` and without the `--type=` helper marker.
fn is_default_main(line: &str, kind: BrowserKind) -> bool {
    if line.contains("--user-data-dir") || line.contains("--type=") {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        line.contains(&format!("{}.app/Contents/MacOS/", kind.bundle()))
    }
    #[cfg(windows)]
    {
        line.to_lowercase().contains(&format!("\\{}", kind.exe_name()))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = kind;
        false
    }
}

/// Command lines of all processes. Windows' `tasklist` cannot show them, so
/// only the launched set is available there.
fn command_lines() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        Command::new("ps")
            .args(["-axo", "command"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Stop the profile's browser: its own instance, or for `Default` the everyday
/// one, which has no `--user-data-dir` to match.
/// `launched_pid` is the process this app started, used on Windows, which
/// cannot list command lines.
pub fn stop(profile: &BrowserProfile, data_dir: &Path, tmp: &Path, launched_pid: Option<u32>) -> bool {
    #[cfg(target_os = "macos")]
    {
        let _ = launched_pid;
        let arg = user_data_arg(profile, data_dir, tmp);
        let Ok(output) = Command::new("ps")
            .args(["-axo", "pid=,command="])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        else {
            return false;
        };
        let listing = String::from_utf8_lossy(&output.stdout);
        let pids = match &arg {
            Some(arg) => main_process_pids(&listing, arg),
            None => default_main_pids(&listing, profile.browser),
        };
        pids.iter().any(|pid| {
            Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (profile, data_dir, tmp);
        let Some(pid) = launched_pid else {
            return false;
        };
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

/// The browser process (no `--type=`) whose command line holds `arg`. Falls
/// back to every match if the main process is not listed.
#[cfg(any(target_os = "macos", test))]
fn main_process_pids(ps_output: &str, arg: &str) -> Vec<u32> {
    main_pids(ps_output, |command| command.contains(arg))
}

/// The everyday browser's processes, matched by name instead of a directory.
#[cfg(target_os = "macos")]
fn default_main_pids(ps_output: &str, kind: BrowserKind) -> Vec<u32> {
    main_pids(ps_output, |command| is_default_main(command, kind))
}

#[cfg(any(target_os = "macos", test))]
fn main_pids(ps_output: &str, wanted: impl Fn(&str) -> bool) -> Vec<u32> {
    let matches: Vec<(u32, bool)> = ps_output
        .lines()
        .filter_map(|line| {
            let (pid, command) = line.trim_start().split_once(char::is_whitespace)?;
            if !wanted(command) {
                return None;
            }
            Some((pid.parse().ok()?, command.contains("--type=")))
        })
        .collect();
    let main: Vec<u32> = matches.iter().filter(|(_, helper)| !helper).map(|(pid, _)| *pid).collect();
    if main.is_empty() {
        matches.into_iter().map(|(pid, _)| pid).collect()
    } else {
        main
    }
}

/// Spawn the profile's browser. With a mark color on macOS the child also
/// gets a CDP debugging pipe (fds 3/4) so the Dock tile can be set; the
/// returned channel is `None` everywhere else.
pub fn spawn(
    profile: &BrowserProfile,
    maps: &[(String, String)],
    data_dir: &Path,
    tmp: &Path,
    mark: Option<[u8; 3]>,
) -> std::io::Result<(Child, Option<crate::dock::Pipe>)> {
    let exe = profile
        .executable()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "browser not found"))?;
    let mut cmd = Command::new(exe);
    cmd.args(launch_args(profile, maps, data_dir, tmp, mark))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let pipe = mark.and_then(|_| crate::dock::prepare(&mut cmd));
    let child = cmd.spawn()?;
    #[cfg(target_os = "macos")]
    let pipe = {
        let mut pipe = pipe;
        if let Some(pipe) = &mut pipe {
            pipe.close_child_ends();
        }
        pipe
    };
    Ok((child, pipe))
}

/// Pre-seed the profile identity Chrome shows in its avatar button and, on
/// Windows, in the taskbar badge. Only written when the profile has no prefs
/// yet, so edits made inside the browser survive.
#[cfg(any(target_os = "macos", windows, test))]
pub fn seed_prefs(profile: &BrowserProfile, color: [u8; 3], user_data: &Path) -> std::io::Result<()> {
    let prefs = user_data.join("Default").join("Preferences");
    if prefs.exists() {
        return Ok(());
    }
    if let Some(parent) = prefs.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(prefs, prefs_json(profile, color))
}

#[cfg(any(target_os = "macos", windows, test))]
fn prefs_json(profile: &BrowserProfile, color: [u8; 3]) -> String {
    let [r, g, b] = color;
    let sk_color = 0xFF00_0000u32 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
    let avatar = TILE_PALETTE.iter().position(|p| *p == color).unwrap_or(0);
    serde_json::json!({
        "profile": {
            "name": profile.name,
            "avatar_index": avatar,
            "profile_color": sk_color,
        }
    })
    .to_string()
}

const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

fn is_stale_name(name: &str) -> bool {
    name.starts_with("shl-ephemeral-")
}

fn is_stale(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified).is_ok_and(|age| age > STALE_AFTER)
}

/// Remove leftover ephemeral dirs. A running browser keeps writing to its
/// dir, so only dirs untouched for a day are considered leftovers.
pub fn clean_stale(tmp: &Path) {
    let Ok(entries) = fs::read_dir(tmp) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_stale_name(&name) || !entry.path().is_dir() {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| is_stale(modified, now));
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Item, ItemKind, Store};

    fn maps() -> Vec<(String, String)> {
        vec![
            ("api.local".into(), "127.0.0.1".into()),
            ("v6.local".into(), "::1".into()),
        ]
    }

    #[test]
    fn mappings_parse_enabled_content_first_host_wins() {
        let content = "# --- Local ---\n127.0.0.1 api.local web.local\n# comment\n\n::1 v6.local\n10.0.0.8 api.local other.local\n";
        assert_eq!(
            mappings(content),
            vec![
                ("api.local".into(), "127.0.0.1".into()),
                ("web.local".into(), "127.0.0.1".into()),
                ("v6.local".into(), "::1".into()),
                ("other.local".into(), "10.0.0.8".into()),
            ]
        );
    }

    #[test]
    fn mappings_skip_comment_tails_and_invalid_lines() {
        let content = "127.0.0.1 api.local # dev box\nnot-an-ip stray.host\n10.0.0.8 ok.local#attached\n";
        assert_eq!(
            mappings(content),
            vec![
                ("api.local".into(), "127.0.0.1".into()),
                ("ok.local".into(), "10.0.0.8".into()),
            ]
        );
    }

    #[test]
    fn mappings_follow_the_system_switches() {
        let mut local = Item::new(ItemKind::Local, "Local");
        local.content = "127.0.0.1 api.local\n".into();
        let id = local.id.clone();
        let mut store = Store {
            items: vec![local],
            multi_select: true,
            ..Store::default()
        };
        assert!(mappings(&store.enabled_content()).is_empty());
        store.set_on(&id, true);
        assert_eq!(
            mappings(&store.enabled_content()),
            vec![("api.local".into(), "127.0.0.1".into())]
        );
    }

    #[test]
    fn ipv6_targets_get_brackets() {
        assert_eq!(format_target("::1"), "[::1]");
        assert_eq!(format_target("127.0.0.1"), "127.0.0.1");
        let profile = BrowserProfile::new("p");
        let cmd = build_cmd(&profile, &maps(), Path::new("/data"), Path::new("/tmp"), None);
        assert!(cmd.contains("MAP v6.local [::1]"));
        assert!(!cmd.contains("MAP v6.local ::1"));
    }

    #[test]
    fn modes_shape_the_command() {
        let maps = vec![("ads.example.com".into(), "0.0.0.0".into())];
        let mut profile = BrowserProfile::new("p");
        profile.id = "p1".into();
        let independent = build_cmd(&profile, &maps, Path::new("/data"), Path::new("/tmp"), None).replace('\\', "/");
        assert!(independent.contains("--user-data-dir=\"/data/browser-profiles/p1\""));
        assert!(!independent.contains("--incognito"));
        profile.mode = ProfileMode::Ephemeral;
        let ephemeral = build_cmd(&profile, &maps, Path::new("/data"), Path::new("/tmp"), None).replace('\\', "/");
        assert!(ephemeral.contains("--incognito --user-data-dir=\"/tmp/shl-ephemeral-p1\""));
        profile.mode = ProfileMode::Default;
        let default = build_cmd(&profile, &maps, Path::new("/data"), Path::new("/tmp"), None);
        assert!(!default.contains("--user-data-dir"));
        assert_eq!(
            launch_args(&profile, &maps, Path::new("/data"), Path::new("/tmp"), None),
            vec![
                "--test-type".to_string(),
                "--host-resolver-rules=MAP ads.example.com 0.0.0.0".to_string()
            ]
        );
    }

    #[test]
    fn manual_path_overrides_detection() {
        let mut profile = BrowserProfile::new("p");
        profile.path = "/opt/custom/chrome".into();
        assert_eq!(profile.executable(), Some(PathBuf::from("/opt/custom/chrome")));
        let cmd = build_cmd(&profile, &[("a".into(), "1.2.3.4".into())], Path::new("/d"), Path::new("/t"), None);
        assert!(cmd.starts_with("\"/opt/custom/chrome\""));
        profile.path = "   ".into();
        assert_eq!(profile.executable(), profile.browser.detected_path());
    }

    #[test]
    fn app_executable_prefers_the_bundle_name() {
        let dir = std::env::temp_dir().join(format!("shl-appexe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let app = dir.join("Fake Browser.app");
        let macos = app.join("Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        assert_eq!(app_executable(&app), None);
        std::fs::write(macos.join("helper"), b"").unwrap();
        std::fs::write(macos.join("Fake Browser"), b"").unwrap();
        assert_eq!(app_executable(&app), Some(macos.join("Fake Browser")));
        std::fs::remove_file(macos.join("Fake Browser")).unwrap();
        assert_eq!(app_executable(&app), Some(macos.join("helper")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn block_reason_priority() {
        let mut profile = BrowserProfile::new("p");
        profile.path = "/bin/true".into();
        assert_eq!(block_reason(&profile, 0, 10, false), Some(Block::NoMappings));
        assert_eq!(block_reason(&profile, 1, cmd_limit() + 1, false), Some(Block::TooLong));
        assert_eq!(block_reason(&profile, 1, 10, true), Some(Block::Running));
        assert_eq!(block_reason(&profile, 1, 10, false), None);
        profile.mode = ProfileMode::Default;
        assert_eq!(block_reason(&profile, 1, 10, true), Some(Block::Running));
        profile.path = String::new();
        profile.browser = BrowserKind::Edge;
        if BrowserKind::Edge.detected_path().is_none() {
            assert_eq!(block_reason(&profile, 1, 10, false), Some(Block::Missing));
            profile.path = "/opt/edge".into();
            assert_eq!(block_reason(&profile, 1, 10, false), None);
        }
    }

    // macOS ps lines and bundle paths; Windows cannot list command lines.
    #[cfg(target_os = "macos")]
    #[test]
    fn running_profiles_match_the_user_data_dir() {
        let mut independent = BrowserProfile::new("a");
        independent.id = "a1".into();
        let mut ephemeral = BrowserProfile::new("b");
        ephemeral.id = "b1".into();
        ephemeral.mode = ProfileMode::Ephemeral;
        let mut everyday = BrowserProfile::new("c");
        everyday.id = "c1".into();
        everyday.mode = ProfileMode::Default;
        let profiles = [independent, ephemeral, everyday];
        let data = Path::new("/data");
        let tmp = Path::new("/tmp");
        let lines = vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome \
             --user-data-dir=/data/browser-profiles/a1 --test-type"
                .to_string(),
        ];
        assert_eq!(
            matching_profiles(&lines, &profiles, data, tmp),
            HashSet::from(["a1".to_string()])
        );
        let lines = vec![
            "Chrome --incognito --user-data-dir=/tmp/shl-ephemeral-b1".to_string(),
            "\"Chrome\" --user-data-dir=\"/data/browser-profiles/a1\"".to_string(),
        ];
        // The quoted form is the copyable command, not a running process.
        assert_eq!(
            matching_profiles(&lines, &profiles, data, tmp),
            HashSet::from(["b1".to_string()])
        );
        assert!(matching_profiles(&lines, &profiles[2..], data, tmp).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_everyday_instance_is_matched_without_a_profile_directory() {
        let mut everyday = BrowserProfile::new("c");
        everyday.id = "c1".into();
        everyday.mode = ProfileMode::Default;
        let profiles = [everyday];
        let (data, tmp) = (Path::new("/data"), Path::new("/tmp"));

        // Another profile's instance always passes its own directory, so it
        // must never make the default profile look open.
        let other = vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome \
             --user-data-dir=/data/browser-profiles/a1"
                .to_string(),
        ];
        assert!(matching_profiles(&other, &profiles, data, tmp).is_empty());

        // Nor do the everyday instance's helpers.
        let helpers = vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --type=renderer".to_string(),
        ];
        assert!(matching_profiles(&helpers, &profiles, data, tmp).is_empty());

        let everyday_line = vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome --restore-last-session"
                .to_string(),
        ];
        assert_eq!(
            matching_profiles(&everyday_line, &profiles, data, tmp),
            HashSet::from(["c1".to_string()])
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stopping_the_default_targets_the_everyday_process() {
        let ps = "  200 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome\n\
                  201 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --type=renderer\n\
                  202 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome \
                      --user-data-dir=/data/browser-profiles/a1\n\
                  203 /Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge\n";
        assert_eq!(default_main_pids(ps, BrowserKind::Chrome), [200]);
        assert_eq!(default_main_pids(ps, BrowserKind::Edge), [203]);
    }

    #[test]
    fn stop_targets_the_browser_process_not_helpers() {
        let ps = "  100 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome \
                  --user-data-dir=/data/browser-profiles/a1\n\
                  101 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome \
                  --type=renderer --user-data-dir=/data/browser-profiles/a1\n\
                  102 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome \
                  --user-data-dir=/data/browser-profiles/other\n";
        assert_eq!(main_process_pids(ps, "--user-data-dir=/data/browser-profiles/a1"), [100]);
        let helpers = "  101 chrome --type=renderer --user-data-dir=/data/p\n";
        assert_eq!(main_process_pids(helpers, "--user-data-dir=/data/p"), [101]);
        assert!(main_process_pids(ps, "--user-data-dir=/nope").is_empty());
    }

    #[test]
    fn snapshot_carries_the_manual_path() {
        let mut profile = BrowserProfile::new("p");
        profile.path = "/opt/chrome".into();
        profile.push_rev(RevKind::Path);
        profile.path = String::new();
        let rev = profile.revs.first().unwrap().clone();
        assert_eq!(rev.snapshot.path, "/opt/chrome");
        profile.apply_snapshot(&rev.snapshot);
        assert_eq!(profile.path, "/opt/chrome");
    }

    #[test]
    fn snapshot_carries_the_switches() {
        let mut profile = BrowserProfile::new("p");
        profile.switches = vec!["a".into()];
        profile.push_rev(RevKind::Selection);
        profile.switches = vec!["b".into()];
        let rev = profile.revs.first().unwrap().clone();
        assert_eq!(rev.snapshot.switches, ["a"]);
        profile.apply_snapshot(&rev.snapshot);
        assert_eq!(profile.switches, ["a"]);
    }

    #[test]
    fn revs_are_capped() {
        let mut profile = BrowserProfile::new("p");
        for _ in 0..(REV_LIMIT + 5) {
            profile.push_rev(RevKind::Mode);
        }
        assert_eq!(profile.revs.len(), REV_LIMIT);
    }

    #[test]
    fn windows_candidates_cover_both_program_files_and_local() {
        let paths = windows_candidates(BrowserKind::Chrome, r"C:\PF", r"C:\PF86", r"C:\Local");
        assert_eq!(paths.len(), 3);
        let first = paths[0].to_string_lossy().replace('\\', "/");
        assert!(first.ends_with("Google/Chrome/Application/chrome.exe"), "{first}");
        assert!(paths[2].starts_with(r"C:\Local"));
        let paths = windows_candidates(BrowserKind::Edge, r"C:\PF", r"C:\PF86", "");
        assert_eq!(paths.len(), 2);
        assert!(paths[0]
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("Microsoft/Edge/Application/msedge.exe"));
    }

    #[test]
    fn legacy_brave_kind_deserializes_as_chrome() {
        let kind: BrowserKind = serde_json::from_str("\"brave\"").unwrap();
        assert_eq!(kind, BrowserKind::Chrome);
    }

    #[test]
    fn stale_scan_only_touches_our_old_dirs() {
        assert!(is_stale_name("shl-ephemeral-p1"));
        assert!(!is_stale_name("shl-update-p1"));
        let now = SystemTime::now();
        assert!(is_stale(now - STALE_AFTER - Duration::from_secs(1), now));
        assert!(!is_stale(now - Duration::from_secs(60), now));
    }

    #[test]
    fn cmd_limit_matches_platform() {
        if cfg!(windows) {
            assert_eq!(cmd_limit(), 32_000);
        } else {
            assert_eq!(cmd_limit(), 262_144);
        }
    }

    #[test]
    fn mark_color_gates_mode() {
        let mut profile = BrowserProfile::new("p");
        profile.tile_color = Some([1, 2, 3]);
        assert_eq!(mark_color(&profile), Some([1, 2, 3]));
        profile.mode = ProfileMode::Default;
        assert_eq!(mark_color(&profile), None);
        profile.tile_color = None;
        profile.mode = ProfileMode::Independent;
        assert_eq!(mark_color(&profile), None);
    }

    #[test]
    fn mark_adds_the_theme_argument_only() {
        let mut profile = BrowserProfile::new("p");
        profile.id = "p1".into();
        let maps = maps();
        let plain = launch_args(&profile, &maps, Path::new("/d"), Path::new("/t"), None);
        assert!(!plain.iter().any(|arg| arg.starts_with("--install-autogenerated-theme")));
        let marked = launch_args(&profile, &maps, Path::new("/d"), Path::new("/t"), Some([255, 90, 0]));
        assert!(marked.contains(&"--install-autogenerated-theme=255,90,0".to_string()));
        let cmd = build_cmd(&profile, &maps, Path::new("/d"), Path::new("/t"), Some([255, 90, 0]));
        assert!(cmd.contains("--install-autogenerated-theme=255,90,0"));
        // The pipe needs fds only this app can provide, so the copyable
        // command stays pasteable without it.
        assert!(!cmd.contains("--remote-debugging-pipe"));
    }

    #[test]
    fn snapshot_carries_the_color() {
        let mut profile = BrowserProfile::new("p");
        profile.tile_color = Some(TILE_PALETTE[2]);
        profile.custom_image = true;
        profile.push_rev(RevKind::Color);
        profile.tile_color = None;
        profile.custom_image = false;
        let rev = profile.revs.first().unwrap().clone();
        assert_eq!(rev.kind, RevKind::Color);
        assert_eq!(rev.snapshot.tile_color, Some(TILE_PALETTE[2]));
        assert!(rev.snapshot.custom_image);
        profile.apply_snapshot(&rev.snapshot);
        assert_eq!(profile.tile_color, Some(TILE_PALETTE[2]));
        assert!(profile.custom_image);
    }

    #[test]
    fn mark_color_round_trips_through_the_picker() {
        for color in TILE_PALETTE {
            assert_eq!(hsla_to_color(color_to_hsla(color)), color);
        }
    }

    #[test]
    fn custom_image_defaults_off_and_is_skipped_when_off() {
        let profile: BrowserProfile = serde_json::from_str(r#"{"id":"p1","name":"p"}"#).unwrap();
        assert!(!profile.custom_image);
        let text = serde_json::to_string(&profile).unwrap();
        assert!(!text.contains("custom_image"));
    }

    #[test]
    fn user_data_dir_follows_the_mode() {
        let mut profile = BrowserProfile::new("p");
        profile.id = "p1".into();
        assert_eq!(
            user_data_dir(&profile, Path::new("/d"), Path::new("/t")),
            Some(PathBuf::from("/d/browser-profiles/p1"))
        );
        profile.mode = ProfileMode::Ephemeral;
        assert_eq!(
            user_data_dir(&profile, Path::new("/d"), Path::new("/t")),
            Some(PathBuf::from("/t/shl-ephemeral-p1"))
        );
        profile.mode = ProfileMode::Default;
        assert_eq!(user_data_dir(&profile, Path::new("/d"), Path::new("/t")), None);
    }

    #[test]
    fn seed_prefs_writes_once_and_keeps_edits() {
        let dir = std::env::temp_dir().join(format!("shl-seed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let profile = BrowserProfile::new("Work");
        seed_prefs(&profile, TILE_PALETTE[1], &dir).unwrap();
        let prefs = dir.join("Default").join("Preferences");
        let text = fs::read_to_string(&prefs).unwrap();
        assert!(text.contains(r#""name":"Work""#), "{text}");
        assert!(text.contains(r#""avatar_index":1"#), "{text}");
        assert!(text.contains(r#""profile_color":4294085672"#), "{text}");
        fs::write(&prefs, "{}").unwrap();
        seed_prefs(&profile, TILE_PALETTE[2], &dir).unwrap();
        assert_eq!(fs::read_to_string(&prefs).unwrap(), "{}");
        let _ = fs::remove_dir_all(&dir);
    }
}