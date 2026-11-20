//! Launch at login: a LaunchAgent plist on macOS, an HKCU Run value on
//! Windows. The preference is the source of truth; `repair` reconciles the
//! system entry with the current executable path at startup.

use anyhow::Result;

use crate::i18n::Localize as _;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const LABEL: &str = "com.switchhostslite.app";

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn plist_xml(exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
        xml_escape(exe)
    )
}

/// The executable the plist launches, `None` when it is not one of ours.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn plist_program(xml: &str) -> Option<String> {
    let args = xml.split("<key>ProgramArguments</key>").nth(1)?;
    let start = args.find("<string>")? + "<string>".len();
    let end = start + args[start..].find("</string>")?;
    Some(xml_unescape(&args[start..end]))
}

/// Run-key command line; the quotes keep spaces in the install path intact.
#[cfg_attr(not(windows), allow(dead_code))]
fn run_command(exe: &str) -> String {
    format!("\"{exe}\"")
}

pub fn set(enabled: bool) -> Result<()> {
    imp::set(enabled).localize("err_autostart")
}

/// Rewrite the system entry when it is missing or points at another copy of
/// the app (moved bundle, new install directory).
pub fn repair() {
    let Ok(exe) = std::env::current_exe() else { return };
    if imp::installed().as_deref() != Some(imp::expected(&exe).as_str())
        && let Err(err) = imp::set(true)
    {
        eprintln!("launch-at-login repair failed: {err:#}");
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{LABEL, plist_program, plist_xml};
    use anyhow::{Result, anyhow};
    use std::path::{Path, PathBuf};

    fn plist_path() -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("no home directory"))?;
        Ok(home.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    pub fn expected(exe: &Path) -> String {
        exe.to_string_lossy().into_owned()
    }

    pub fn installed() -> Option<String> {
        let xml = std::fs::read_to_string(plist_path().ok()?).ok()?;
        plist_program(&xml)
    }

    pub fn set(enabled: bool) -> Result<()> {
        let path = plist_path()?;
        if enabled {
            let exe = std::env::current_exe()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, plist_xml(&expected(&exe)))?;
        } else {
            match std::fs::remove_file(&path) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use super::run_command;
    use anyhow::{Result, anyhow};
    use std::path::Path;
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS},
        System::Registry::{
            HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW,
            RegDeleteValueW, RegGetValueW, RegSetValueExW,
        },
    };

    const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "SwitchHostsLite";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    pub fn expected(exe: &Path) -> String {
        run_command(&exe.to_string_lossy())
    }

    fn open_write_key() -> Result<*mut core::ffi::c_void> {
        let key = wide(RUN_KEY);
        let mut hkey = core::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                core::ptr::null(),
                0,
                KEY_SET_VALUE,
                core::ptr::null(),
                &mut hkey,
                core::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(anyhow!("open Run key failed: {status}"));
        }
        Ok(hkey)
    }

    pub fn installed() -> Option<String> {
        let key = wide(RUN_KEY);
        let name = wide(VALUE_NAME);
        let mut buf = vec![0u16; 512];
        loop {
            let mut size = u32::try_from(buf.len() * 2).ok()?;
            let status = unsafe {
                RegGetValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    RRF_RT_REG_SZ,
                    core::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            match status {
                ERROR_SUCCESS => {
                    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    return String::from_utf16(&buf[..len]).ok();
                }
                ERROR_MORE_DATA => buf.resize((size as usize).div_ceil(2).max(buf.len() + 1), 0),
                _ => return None,
            }
        }
    }

    pub fn set(enabled: bool) -> Result<()> {
        let hkey = open_write_key()?;
        let name = wide(VALUE_NAME);
        let status = if enabled {
            let exe = std::env::current_exe()?;
            let data = wide(&expected(&exe));
            unsafe {
                RegSetValueExW(
                    hkey,
                    name.as_ptr(),
                    0,
                    REG_SZ,
                    data.as_ptr().cast(),
                    u32::try_from(data.len() * 2).unwrap_or(u32::MAX),
                )
            }
        } else {
            unsafe { RegDeleteValueW(hkey, name.as_ptr()) }
        };
        unsafe { RegCloseKey(hkey) };
        match status {
            ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(()),
            status => Err(anyhow!("write Run value failed: {status}")),
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use anyhow::Result;
    use std::path::Path;

    pub fn expected(_exe: &Path) -> String {
        String::new()
    }

    pub fn installed() -> Option<String> {
        None
    }

    pub fn set(_enabled: bool) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_round_trips_escaped_paths() {
        let exe = "/Users/a&b/<x>/SwitchHosts Lite.app/Contents/MacOS/switch-hosts-lite";
        let xml = plist_xml(exe);
        assert!(xml.contains(LABEL) && xml.contains("<key>RunAtLoad</key>"));
        assert!(xml.contains("a&amp;b/&lt;x&gt;") && !xml.contains("a&b"));
        assert_eq!(plist_program(&xml).as_deref(), Some(exe));
    }

    #[test]
    fn plist_program_needs_the_arguments_key() {
        assert_eq!(plist_program("<plist><dict></dict></plist>"), None);
        assert_eq!(plist_program("<key>ProgramArguments</key><array></array>"), None);
    }

    #[test]
    fn run_command_quotes_spaces() {
        assert_eq!(
            run_command(r"C:\Program Files\SwitchHostsLite\switch-hosts-lite.exe"),
            r#""C:\Program Files\SwitchHostsLite\switch-hosts-lite.exe""#
        );
    }
}