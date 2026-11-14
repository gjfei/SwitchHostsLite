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

/// Argument that marks the elevated child started by `write_elevated`:
/// `--elevated-write-hosts <staged file>`.
#[cfg(any(windows, test))]
const ELEVATED_WRITE_ARG: &str = "--elevated-write-hosts";

/// The command line of the elevated child, besides the program itself.
#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
enum ElevatedArgs {
    /// An ordinary launch.
    Normal,
    /// The helper was started with a malformed command line.
    Invalid,
    /// Replace the system hosts with the staged file.
    Write(PathBuf),
}

/// Only exactly the flag and its path count, so an ordinary launch, also with
/// other arguments, never writes the hosts.
#[cfg(any(windows, test))]
fn elevated_args<I, S>(args: I) -> ElevatedArgs
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    if args
        .get(1)
        .is_none_or(|arg| arg.as_os_str() != std::ffi::OsStr::new(ELEVATED_WRITE_ARG))
    {
        return ElevatedArgs::Normal;
    }
    match args.as_slice() {
        [_, _, src] => ElevatedArgs::Write(PathBuf::from(src)),
        _ => ElevatedArgs::Invalid,
    }
}

/// When this process is the elevated helper started by `write_elevated`, write
/// the staged hosts and exit. An ordinary launch returns immediately.
#[cfg(windows)]
pub fn run_as_elevated_helper() {
    match elevated_args(std::env::args_os()) {
        ElevatedArgs::Normal => {}
        ElevatedArgs::Invalid => {
            eprintln!("elevated helper: expected {ELEVATED_WRITE_ARG} <staged hosts>");
            std::process::exit(1);
        }
        ElevatedArgs::Write(src) => {
            let code = match replace_hosts_from(&src) {
                Ok(()) => 0,
                Err(err) => {
                    eprintln!("elevated helper: {err:#}");
                    1
                }
            };
            std::process::exit(code);
        }
    }
}

#[cfg(windows)]
fn replace_hosts_from(src: &Path) -> Result<()> {
    let content = fs::read(src).localize("err_write_hosts")?;
    fs::write(hosts_path()?, content).localize("err_write_hosts")?;
    flush_dns();
    Ok(())
}

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

/// Overwrite the hosts from an elevated copy of this executable, which shows
/// the UAC prompt, then check that the hosts holds what was intended.
#[cfg(windows)]
fn write_elevated(content: &str) -> Result<()> {
    let hosts = hosts_path()?;
    let tmp = write_tmp(content)?;
    let result = run_elevated(&tmp);
    let _ = fs::remove_file(&tmp);
    result?;
    if fs::read(hosts).ok().as_deref() != Some(content.as_bytes()) {
        return Err(LocalizedError::new("err_hosts_mismatch").into());
    }
    Ok(())
}

/// Start this executable with the `runas` verb and wait for it. The child is
/// the elevated helper of `run_as_elevated_helper`; Windows shows the UAC
/// prompt and its dialog reports a refused elevation.
#[cfg(windows)]
fn run_elevated(src: &Path) -> Result<()> {
    use std::{
        ffi::{OsStr, OsString},
        mem,
        os::windows::ffi::OsStrExt as _,
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError, WAIT_OBJECT_0},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject},
        },
        UI::{
            Shell::{
                SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
                ShellExecuteExW,
            },
            WindowsAndMessaging::SW_HIDE,
        },
    };

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain([0]).collect()
    }

    let exe = std::env::current_exe()
        .map_err(|err| LocalizedError::new("err_auth_failed").detail(err))?;
    // Windows splits this with CommandLineToArgvW, which, unlike `cmd`, does
    // not expand `%`, so a temp path containing one survives. NTFS forbids `"`
    // in names, so the surrounding quotes cannot be escaped away.
    let mut params = OsString::from(ELEVATED_WRITE_ARG);
    params.push(" \"");
    params.push(src.as_os_str());
    params.push("\"");
    let verb = wide(OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    let params = wide(&params);

    // ShellExecuteExW needs an STA thread. A fresh one keeps the UI thread's
    // COM state untouched and matches the synchronous write path.
    std::thread::scope(|scope| {
        scope
            .spawn(move || unsafe {
                let hr = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
                let com = hr >= 0;
                let mut info: SHELLEXECUTEINFOW = mem::zeroed();
                info.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
                info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
                info.lpVerb = verb.as_ptr();
                info.lpFile = file.as_ptr();
                info.lpParameters = params.as_ptr();
                info.nShow = SW_HIDE;

                if ShellExecuteExW(&mut info) == 0 {
                    let err = GetLastError();
                    if com {
                        CoUninitialize();
                    }
                    return Err(if err == ERROR_CANCELLED {
                        LocalizedError::new("err_auth_cancelled").into()
                    } else {
                        LocalizedError::new("err_auth_failed")
                            .detail(format!("ShellExecuteExW: error {err}"))
                            .into()
                    });
                }
                let process = info.hProcess;
                if process.is_null() {
                    if com {
                        CoUninitialize();
                    }
                    return Err(LocalizedError::new("err_auth_failed")
                        .detail("no process handle")
                        .into());
                }
                let wait = WaitForSingleObject(process, INFINITE);
                let mut code = 0u32;
                let exit_known =
                    wait == WAIT_OBJECT_0 && GetExitCodeProcess(process, &mut code) != 0;
                CloseHandle(process);
                if com {
                    CoUninitialize();
                }
                if !exit_known {
                    return Err(LocalizedError::new("err_auth_failed")
                        .detail(format!("WaitForSingleObject: {wait}"))
                        .into());
                }
                if code == 0 {
                    Ok(())
                } else {
                    Err(LocalizedError::new("err_write_hosts")
                        .detail(format!("elevated helper exit code {code}"))
                        .into())
                }
            })
            .join()
            .map_err(|_| LocalizedError::new("err_auth_failed"))?
    })
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
    fn elevated_args_only_accept_the_helper_flag_and_one_path() {
        let helper = |args: &[&str]| elevated_args(args.iter().copied());

        assert_eq!(helper(&["switch-hosts-lite.exe"]), ElevatedArgs::Normal);
        assert_eq!(helper(&["switch-hosts-lite.exe", "--other"]), ElevatedArgs::Normal);
        assert_eq!(helper(&["switch-hosts-lite.exe", "a", ELEVATED_WRITE_ARG]), ElevatedArgs::Normal);

        assert_eq!(helper(&["switch-hosts-lite.exe", ELEVATED_WRITE_ARG]), ElevatedArgs::Invalid);
        assert_eq!(helper(&["switch-hosts-lite.exe", ELEVATED_WRITE_ARG, "a", "b"]), ElevatedArgs::Invalid);

        assert_eq!(
            helper(&["switch-hosts-lite.exe", ELEVATED_WRITE_ARG, r"C:\Temp\x hosts"]),
            ElevatedArgs::Write(PathBuf::from(r"C:\Temp\x hosts"))
        );
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
