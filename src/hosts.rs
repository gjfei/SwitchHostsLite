use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::Result;

use crate::i18n::{Localize as _, LocalizedError};

const START: &str = "# --- SWITCHHOSTS_LITE_CONTENT_START ---";
const END: &str = "# --- SWITCHHOSTS_LITE_CONTENT_END ---";

/// The system hosts file, resolved once.
#[cfg(windows)]
pub fn hosts_path() -> Result<&'static Path, LocalizedError> {
    static PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        select_hosts_path(win::database_dir(), |path| Path::new(path).is_file(), win::windows_dir)
            .map(PathBuf::from)
    })
    .as_deref()
    .ok_or_else(|| LocalizedError::new("err_hosts_path_unknown"))
}

#[cfg(not(windows))]
pub fn hosts_path() -> Result<&'static Path, LocalizedError> {
    Ok(Path::new("/etc/hosts"))
}

/// The hosts in the TCP/IP `DataBasePath` directory when that file exists,
/// as that is where Windows reads it from, else the one under the Windows
/// directory.
#[cfg(any(windows, test))]
fn select_hosts_path(
    database_dir: Option<String>,
    is_file: impl Fn(&str) -> bool,
    windows_dir: impl FnOnce() -> Option<String>,
) -> Option<String> {
    database_dir
        .filter(|dir| !dir.trim().is_empty())
        .map(|dir| join_windows(&dir, "hosts"))
        .filter(|path| is_file(path))
        .or_else(|| {
            windows_dir()
                .filter(|dir| !dir.trim().is_empty())
                .map(|dir| join_windows(&dir, r"System32\drivers\etc\hosts"))
        })
}

/// `rest` inside the Windows directory `dir`. A bare drive such as `D:`
/// gets its root, as `D:hosts` would be relative to the drive's current
/// directory.
#[cfg(any(windows, test))]
fn join_windows(dir: &str, rest: &str) -> String {
    if dir.ends_with(['\\', '/']) {
        format!("{dir}{rest}")
    } else {
        format!("{dir}\\{rest}")
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::{
        Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS},
        System::{
            Environment::ExpandEnvironmentStringsW,
            Registry::{HKEY_LOCAL_MACHINE, REG_EXPAND_SZ, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW},
            SystemInformation::GetSystemWindowsDirectoryW,
        },
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// The text of `buf` up to its first NUL.
    fn text(buf: &[u16]) -> Option<String> {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16(&buf[..len]).ok()
    }

    /// `DataBasePath` of the TCP/IP parameters with environment variables expanded.
    pub fn database_dir() -> Option<String> {
        let key = wide(r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters");
        let name = wide("DataBasePath");
        let mut buf = vec![0u16; 260];
        loop {
            let mut kind = 0;
            let mut size = u32::try_from(buf.len() * 2).ok()?;
            let status = unsafe {
                RegGetValueW(
                    HKEY_LOCAL_MACHINE,
                    key.as_ptr(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
                    &mut kind,
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    buf.truncate(size as usize / 2);
                    return if kind == REG_EXPAND_SZ { expand(&buf) } else { text(&buf) };
                }
                ERROR_MORE_DATA => buf.resize((size as usize).div_ceil(2).max(buf.len() + 1), 0),
                _ => return None,
            }
        }
    }

    fn expand(value: &[u16]) -> Option<String> {
        let len = value.iter().position(|&c| c == 0).unwrap_or(value.len());
        let src: Vec<u16> = value[..len].iter().copied().chain([0]).collect();
        let mut buf = vec![0u16; 260];
        loop {
            let size = u32::try_from(buf.len()).ok()?;
            // The returned length includes the NUL, also when the buffer is too small.
            match unsafe { ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), size) } as usize {
                0 => return None,
                n if n <= buf.len() => return text(&buf[..n]),
                n => buf.resize(n, 0),
            }
        }
    }

    pub fn windows_dir() -> Option<String> {
        let mut buf = vec![0u16; 260];
        loop {
            let size = u32::try_from(buf.len()).ok()?;
            // The length excludes the NUL on success and includes it when the buffer is too small.
            match unsafe { GetSystemWindowsDirectoryW(buf.as_mut_ptr(), size) } as usize {
                0 => return None,
                n if n < buf.len() => return text(&buf[..n]),
                n => buf.resize(n, 0),
            }
        }
    }
}

/// Text as handled inside the app: LF line ends and no byte order mark.
fn normalize(text: &str) -> String {
    text.strip_prefix('\u{feff}').unwrap_or(text).replace("\r\n", "\n")
}

/// `text` (LF line ends) with CRLF line ends when `crlf` is set.
fn with_line_endings(text: &str, crlf: bool) -> String {
    if crlf {
        text.replace("\r\n", "\n").replace('\n', "\r\n")
    } else {
        text.to_string()
    }
}

/// The bytes written to disk use the platform's line ends.
fn to_native(text: &str) -> String {
    with_line_endings(text, cfg!(windows))
}

/// Modification time and length of a file, to notice changes without reading it.
pub type Stamp = (std::time::SystemTime, u64);

/// The current stamp of the system hosts, `None` when it cannot be read.
pub fn system_stamp() -> Option<Stamp> {
    let meta = fs::metadata(hosts_path().ok()?).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// The system hosts for display. Unreadable or non UTF-8 parts are replaced.
pub fn read_system() -> String {
    hosts_path()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| normalize(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_default()
}

/// The system hosts as the base of a rewrite. Fails rather than returning
/// something that would drop the user's own entries.
fn read_current() -> Result<String> {
    match fs::read(hosts_path()?) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(|text| normalize(&text))
            .map_err(|_| LocalizedError::new("err_hosts_not_utf8").into()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err).localize("err_read_hosts"),
    }
}

/// The system hosts with the block managed by this app removed. Lines after
/// a start marker without an end marker are kept, as they may be the user's.
fn strip_managed(text: &str) -> String {
    let mut out = Vec::new();
    // Index in `out` of the start marker of the open block.
    let mut block = None;
    for line in text.lines() {
        match (line.trim(), block) {
            (START, _) => {
                block = Some(out.len());
                out.push(line);
            }
            (END, Some(start)) => {
                out.truncate(start);
                block = None;
            }
            (END, None) => {}
            _ => out.push(line),
        }
    }
    out.join("\n").trim_end().to_string()
}

/// Keep the user's original hosts untouched and replace only our managed block.
pub fn build(system: &str, enabled: &str) -> String {
    let base = strip_managed(system);
    if enabled.trim().is_empty() {
        return format!("{base}\n");
    }
    format!("{base}\n\n{START}\n{}\n{END}\n", enabled.trim_end())
}

/// Apply the enabled items to the system hosts file and return the content
/// before and after the write.
pub fn apply(enabled: &str) -> Result<(String, String)> {
    let current = read_current()?;
    let next = build(&current, enabled);
    if next != current {
        write_raw(&next)?;
    }
    Ok((current, next))
}

/// Overwrite the system hosts file, asking for administrator privileges when
/// a direct write is not permitted.
pub fn write_raw(content: &str) -> Result<()> {
    let content = to_native(content);
    match fs::write(hosts_path()?, &content) {
        Ok(()) => {
            #[cfg(windows)]
            flush_dns();
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => write_elevated(&content),
        Err(err) if is_locked(&err) => Err(err).localize("err_hosts_locked"),
        Err(err) => Err(err).localize("err_write_hosts"),
    }
}

/// Sharing or lock violation, as caused by antivirus software guarding the hosts on Windows.
fn is_locked(err: &io::Error) -> bool {
    cfg!(windows) && matches!(err.raw_os_error(), Some(32 | 33))
}

/// Whether the hosts file can be written without asking for privileges.
pub fn can_write_directly() -> bool {
    hosts_path().is_ok_and(|path| fs::OpenOptions::new().append(true).open(path).is_ok())
}

/// Whether the system hosts already reflects `enabled`.
pub fn is_applied(system: &str, enabled: &str) -> bool {
    build(system, enabled).trim_end() == system.trim_end()
}

fn write_tmp(content: &str) -> Result<PathBuf> {
    let tmp = std::env::temp_dir().join("switch-hosts-lite.hosts");
    fs::write(&tmp, content).localize("err_write_temp")?;
    Ok(tmp)
}

#[cfg(target_os = "macos")]
fn write_elevated(content: &str) -> Result<()> {
    let tmp = write_tmp(content)?;
    let shell = format!(
        "cat '{}' > /etc/hosts && dscacheutil -flushcache; killall -HUP mDNSResponder 2>/dev/null; true",
        tmp.display()
    );
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        shell.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|err| LocalizedError::new("err_launch").arg("program", "osascript").detail(err))?;
    let _ = fs::remove_file(&tmp);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // -128 is userCanceledErr, the password prompt was dismissed.
        if stderr.contains("(-128)") {
            return Err(LocalizedError::new("err_auth_cancelled").into());
        }
        return Err(LocalizedError::new("err_auth_failed").detail(stderr.trim()).into());
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn write_elevated(content: &str) -> Result<()> {
    let tmp = write_tmp(content)?;
    let status = std::process::Command::new("pkexec")
        .args(["sh", "-c", &format!("cat '{}' > /etc/hosts", tmp.display())])
        .status()
        .map_err(|err| LocalizedError::new("err_launch").arg("program", "pkexec").detail(err))?;
    let _ = fs::remove_file(&tmp);
    match status.code() {
        Some(0) => {}
        // pkexec exits with 126 when the authentication dialog was dismissed.
        Some(126) => return Err(LocalizedError::new("err_auth_cancelled").into()),
        code => {
            let code = code.map_or_else(|| "?".to_string(), |c| c.to_string());
            return Err(LocalizedError::new("err_auth_failed_code").arg("code", code).into());
        }
    }
    Ok(())
}

/// Exit code of the elevation script when the UAC prompt was declined (ERROR_CANCELLED).
#[cfg(windows)]
const UAC_CANCELLED: i32 = 1223;

/// A process that does not flash a console window.
#[cfg(windows)]
fn hidden_command(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(windows)]
fn flush_dns() {
    let _ = hidden_command("ipconfig").arg("/flushdns").output();
}

/// Copy a temp file over the hosts from an elevated PowerShell, which shows
/// the UAC prompt, then check that the hosts holds what was intended.
#[cfg(windows)]
fn write_elevated(content: &str) -> Result<()> {
    let hosts = hosts_path()?;
    let tmp = write_tmp(content)?;
    let script = elevation_script(&tmp.to_string_lossy(), &hosts.to_string_lossy());
    let output = hidden_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
        .arg(encode_command(&script))
        .output();
    let _ = fs::remove_file(&tmp);
    let output = output.map_err(|err| LocalizedError::new("err_launch").arg("program", "PowerShell").detail(err))?;
    match output.status.code() {
        Some(0) => {}
        Some(UAC_CANCELLED) => return Err(LocalizedError::new("err_auth_cancelled").into()),
        code => {
            let code = code.map_or_else(|| "?".to_string(), |c| c.to_string());
            return Err(LocalizedError::new("err_auth_failed_code")
                .arg("code", code)
                .detail(String::from_utf8_lossy(&output.stderr).trim())
                .into());
        }
    }
    if fs::read(hosts).ok().as_deref() != Some(content.as_bytes()) {
        return Err(LocalizedError::new("err_hosts_mismatch").into());
    }
    Ok(())
}

/// PowerShell that runs the copy elevated and exits with its exit code, or
/// with `UAC_CANCELLED` when elevation was refused.
#[cfg(any(windows, test))]
fn elevation_script(tmp: &str, hosts: &str) -> String {
    let copy = format!(
        "$ErrorActionPreference = 'Stop'; \
         Copy-Item -LiteralPath {} -Destination {} -Force; \
         ipconfig /flushdns | Out-Null; exit 0",
        ps_quote(tmp),
        ps_quote(hosts)
    );
    format!(
        "try {{ $p = Start-Process -FilePath powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{}' }} \
         catch {{ exit 1223 }}; exit $p.ExitCode",
        encode_command(&copy)
    )
}

/// A PowerShell single-quoted string literal. PowerShell also treats the
/// typographic single quotes as quote characters, so those are doubled too.
#[cfg(any(windows, test))]
fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// Argument for `powershell -EncodedCommand`: base64 of the UTF-16LE script.
#[cfg(any(windows, test))]
fn encode_command(script: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_replaces_managed_block() {
        let system = "127.0.0.1 localhost\n";
        let once = build(system, "1.1.1.1 a.test");
        assert!(once.contains(START) && once.contains("1.1.1.1 a.test"));

        let twice = build(&once, "2.2.2.2 b.test");
        assert!(!twice.contains("a.test"));
        assert_eq!(twice.matches(START).count(), 1);

        assert_eq!(build(&twice, ""), system);
    }

    #[test]
    fn unterminated_block_keeps_following_lines() {
        let system = format!("127.0.0.1 localhost\n{START}\n1.1.1.1 a.test\n10.0.0.1 mine\n");
        let next = build(&system, "2.2.2.2 b.test");
        assert!(next.contains("10.0.0.1 mine") && next.contains("2.2.2.2 b.test"));
        assert_eq!(build(&next, "3.3.3.3 c.test"), build(&system, "3.3.3.3 c.test"));
    }

    #[test]
    fn normalize_strips_bom_and_crlf() {
        assert_eq!(normalize("\u{feff}127.0.0.1 a\r\n::1 b\r\n"), "127.0.0.1 a\n::1 b\n");
        assert_eq!(normalize("1.1.1.1 a\n"), "1.1.1.1 a\n");
        assert_eq!(normalize("a\u{feff}b"), "a\u{feff}b");
    }

    #[test]
    fn line_endings_for_output() {
        assert_eq!(with_line_endings("a\nb\n", true), "a\r\nb\r\n");
        assert_eq!(with_line_endings("a\r\nb\n", true), "a\r\nb\r\n");
        assert_eq!(with_line_endings("a\nb\n", false), "a\nb\n");
        assert_eq!(to_native("a\n"), if cfg!(windows) { "a\r\n" } else { "a\n" });
    }

    #[test]
    fn managed_block_on_crlf_input() {
        let windows = format!("\u{feff}127.0.0.1 localhost\r\n\r\n{START}\r\n1.1.1.1 a.test\r\n{END}\r\n");
        let system = normalize(&windows);
        let next = build(&system, "2.2.2.2 b.test");
        assert_eq!(next, format!("127.0.0.1 localhost\n\n{START}\n2.2.2.2 b.test\n{END}\n"));
        assert!(is_applied(&normalize(&with_line_endings(&next, true)), "2.2.2.2 b.test"));
        assert_eq!(build(&windows, ""), "\u{feff}127.0.0.1 localhost\n");
        assert_eq!(
            with_line_endings(&build(&system, ""), true),
            "127.0.0.1 localhost\r\n"
        );
    }

    #[test]
    fn powershell_quoting_and_encoding() {
        assert_eq!(ps_quote(r"C:\Windows\hosts"), r"'C:\Windows\hosts'");
        assert_eq!(ps_quote("C:\\Users\\O'Brien\\x"), "'C:\\Users\\O''Brien\\x'");
        assert_eq!(ps_quote("a\u{2019}b"), "'a\u{2019}\u{2019}b'");
        assert_eq!(ps_quote("$env:x `y"), "'$env:x `y'");

        // `[Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes(...))`
        assert_eq!(encode_command("exit"), "ZQB4AGkAdAA=");
        assert_eq!(encode_command("ab"), "YQBiAA==");
        assert_eq!(encode_command(""), "");

        let script = elevation_script(r"C:\Temp\x'.hosts", r"C:\Windows\System32\drivers\etc\hosts");
        assert!(script.contains("-Verb RunAs") && script.contains("exit 1223"));
        assert!(!script.contains('"'));
    }

    #[test]
    fn windows_hosts_path_joins() {
        let hosts = r"System32\drivers\etc\hosts";
        assert_eq!(join_windows(r"D:\Windows", hosts), r"D:\Windows\System32\drivers\etc\hosts");
        assert_eq!(join_windows("D:", hosts), r"D:\System32\drivers\etc\hosts");
        assert_eq!(join_windows(r"D:\", hosts), r"D:\System32\drivers\etc\hosts");
        assert_eq!(
            join_windows(r"E:\Win\System32\drivers\etc", "hosts"),
            r"E:\Win\System32\drivers\etc\hosts"
        );
        assert_eq!(join_windows(r"\\srv\etc\", "hosts"), r"\\srv\etc\hosts");
    }

    #[test]
    fn windows_hosts_path_selection() {
        let etc = || Some(r"E:\Data\etc".to_string());
        let windows = || Some(r"D:\Windows".to_string());
        let from_windows = Some(r"D:\Windows\System32\drivers\etc\hosts".to_string());

        let exists = |path: &str| path == r"E:\Data\etc\hosts";
        assert_eq!(select_hosts_path(etc(), exists, || unreachable!()).as_deref(), Some(r"E:\Data\etc\hosts"));
        assert_eq!(select_hosts_path(etc(), |_| false, windows), from_windows);
        assert_eq!(select_hosts_path(None, |_| true, windows), from_windows);
        assert_eq!(select_hosts_path(Some(" ".into()), |_| true, windows), from_windows);
        assert_eq!(select_hosts_path(Some(String::new()), |_| true, || Some("D:".into())).as_deref(), Some(r"D:\System32\drivers\etc\hosts"));
        assert_eq!(select_hosts_path(etc(), |_| false, || None), None);
        assert_eq!(select_hosts_path(None, |_| true, || Some(String::new())), None);
    }
}
