//! Windows platform glue: an image-only file picker, the best-effort hand-off
//! of the composed picture to the browser window's icon, and the graceful
//! window close used to stop a profile.
//!
//! gpui's own path prompt cannot filter by file type, so the picker here is a
//! `GetOpenFileNameW` dialog with an image filter, run on its own thread
//! because it blocks until the user answers. Chrome draws its own taskbar
//! button from the window icon, so `WM_SETICON` is attempted while the window
//! appears and simply ignored when the browser repaints over it.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rust_i18n::t;
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, WAIT_OBJECT_0};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, EnumWindows, GetClassNameW, GetForegroundWindow,
    GetWindowThreadProcessId, ICON_BIG, ICON_SMALL, IsWindowVisible, PostMessageW, SendMessageW,
    WM_CLOSE, WM_SETICON,
};

/// Windows caps `lpstrFile` at `MAX_PATH` for this dialog.
const MAX_PATH: usize = 260;
/// How long the window-icon hand-off keeps trying, at [`RETRY`] intervals:
/// Chrome creates its window a moment after the process starts.
const RETRY: Duration = Duration::from_millis(400);
const RETRIES: u32 = 30;

/// Ask for an image file, returning its path. Blocks on a worker thread, so the
/// caller awaits a channel instead of the UI event loop.
pub fn prompt_for_image(prompt: &str) -> impl Future<Output = Option<PathBuf>> + use<> {
    let (sender, receiver) = futures_channel::oneshot::channel();
    let title: Vec<u16> = prompt.encode_utf16().chain(Some(0)).collect();
    std::thread::spawn(move || {
        let picked = open_panel(&title);
        let _ = sender.send(picked);
    });
    async move { receiver.await.ok().flatten() }
}

fn open_panel(title: &[u16]) -> Option<PathBuf> {
    // Double-NUL terminated, as the API expects.
    let filter: Vec<u16> = format!(
        "{} (*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif)\0*.png;*.jpg;*.jpeg;*.webp;*.bmp;*.gif\0{} (*.*)\0*.*\0\0",
        t!("picker_images"),
        t!("picker_all_files"),
    )
    .encode_utf16()
    .collect();
    let mut buffer = [0u16; MAX_PATH];

    // SAFETY: every pointer handed to the dialog outlives the call, and the
    // buffer is writable for the size it is announced with.
    unsafe {
        // The shell's dialogs want an apartment; a second initialisation on
        // this thread simply fails and is ignored.
        let com = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let mut dialog: OPENFILENAMEW = std::mem::zeroed();
        dialog.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = GetForegroundWindow();
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = buffer.as_mut_ptr();
        dialog.nMaxFile = buffer.len() as u32;
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
        let accepted = GetOpenFileNameW(&mut dialog) != 0;
        if com >= 0 {
            CoUninitialize();
        }
        if !accepted {
            return None;
        }
    }

    let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(0);
    let path = String::from_utf16_lossy(&buffer[..length]);
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Try to give the launched browser the composed picture as its window icon.
/// Runs on its own thread and gives up silently; nothing depends on it.
pub fn hand_off_window_icon(pid: u32, tile: &Path) {
    let tile = tile.to_path_buf();
    std::thread::spawn(move || {
        for _ in 0..RETRIES {
            if let Ok(png) = std::fs::read(&tile)
                && set_window_icon(pid, &png)
            {
                return;
            }
            std::thread::sleep(RETRY);
        }
    });
}

struct Target {
    pid: u32,
    icon: *mut c_void,
    set: bool,
}

fn set_window_icon(pid: u32, png: &[u8]) -> bool {
    // SAFETY: the icon is created from a PNG buffer that outlives the call, and
    // the enumeration context lives until `EnumWindows` returns.
    unsafe {
        // A PNG-compressed icon resource: `cxDesired`/`cyDesired` of 0 keep the
        // 256 px stored size, which Windows scales down for the taskbar.
        let icon = CreateIconFromResourceEx(png.as_ptr(), png.len() as u32, 1, 0x0003_0000, 0, 0, 0);
        if icon.is_null() {
            return false;
        }
        let mut target = Target {
            pid,
            icon,
            set: false,
        };
        EnumWindows(
            Some(apply_icon),
            &mut target as *mut Target as LPARAM,
        );
        // A handed-off icon stays alive because the window keeps drawing from
        // it; only the retries that reached no window release theirs.
        if !target.set {
            DestroyIcon(icon);
        }
        target.set
    }
}

/// `EnumWindows` callback: give the process's visible browser window the icon.
unsafe extern "system" fn apply_icon(window: HWND, lparam: LPARAM) -> BOOL {
    let target = unsafe { &mut *(lparam as *mut Target) };
    let mut pid = 0u32;
    // SAFETY: both calls only read the window and write the locals above.
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
        if pid != target.pid || IsWindowVisible(window) == 0 {
            return 1;
        }
        let mut class = [0u16; 64];
        let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        if length <= 0 {
            return 1;
        }
        // Chrome's top-level windows are `Chrome_WidgetWin_1`; tool windows
        // share the process and must keep their own icon.
        if !String::from_utf16_lossy(&class[..length as usize]).starts_with("Chrome_WidgetWin") {
            return 1;
        }
        SendMessageW(window, WM_SETICON, ICON_BIG as usize, target.icon as isize);
        SendMessageW(window, WM_SETICON, ICON_SMALL as usize, target.icon as isize);
    }
    target.set = true;
    0
}

/// Ask the process's browser windows to close, so Chrome shuts down gracefully
/// and flushes `Preferences` (the exit read-back depends on it). Returns how
/// many windows were asked to close; 0 means there was nothing to close.
pub fn close_browser_windows(pid: u32) -> usize {
    let mut target = CloseTarget { pid, closed: 0 };
    // SAFETY: the enumeration context lives until `EnumWindows` returns.
    unsafe {
        EnumWindows(Some(close_window), &mut target as *mut CloseTarget as LPARAM);
    }
    target.closed
}

struct CloseTarget {
    pid: u32,
    closed: usize,
}

/// `EnumWindows` callback: post `WM_CLOSE` to each visible browser window of
/// the process. Unlike the icon hand-off it keeps enumerating, because a
/// profile may have several windows and Chrome exits only once all close.
unsafe extern "system" fn close_window(window: HWND, lparam: LPARAM) -> BOOL {
    let target = unsafe { &mut *(lparam as *mut CloseTarget) };
    let mut pid = 0u32;
    // SAFETY: both calls only read the window and write the locals above.
    unsafe {
        GetWindowThreadProcessId(window, &mut pid);
        if pid != target.pid || IsWindowVisible(window) == 0 {
            return 1;
        }
        let mut class = [0u16; 64];
        let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        if length <= 0 {
            return 1;
        }
        if !String::from_utf16_lossy(&class[..length as usize]).starts_with("Chrome_WidgetWin") {
            return 1;
        }
        PostMessageW(window, WM_CLOSE, 0, 0);
    }
    target.closed += 1;
    1
}

/// Wait up to `timeout` for the process to exit, reporting whether it did.
/// A synchronize-only handle is enough to wait on it and is closed afterwards.
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    // SAFETY: the handle is opened for this pid, waited on once, then closed.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let waited = WaitForSingleObject(handle, timeout.as_millis() as u32);
        CloseHandle(handle);
        waited == WAIT_OBJECT_0
    }
}
