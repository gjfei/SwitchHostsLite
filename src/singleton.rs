//! One app instance per user session. The gate runs before GPUI so a second
//! launch cannot load data, register another tray icon or start background work.

use std::io;

#[cfg(target_os = "macos")]
use std::fs::File;

#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;
#[cfg(windows)]
pub(crate) use windows::{CLASS, SHOW_MESSAGE, TITLE};

pub struct InstanceGuard {
    #[cfg(target_os = "macos")]
    _file: File,
    #[cfg(windows)]
    _handle: HANDLE,
}

pub fn acquire() -> io::Result<Option<InstanceGuard>> {
    #[cfg(target_os = "macos")]
    {
        macos::acquire()
    }
    #[cfg(windows)]
    {
        windows::acquire()
    }
}

#[cfg(target_os = "macos")]
pub fn watch(cx: &mut gpui_kit::App) {
    macos::watch(cx);
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_char, c_void};
    use std::fs::{OpenOptions, TryLockError};
    use std::io;
    use std::os::unix::fs::OpenOptionsExt;
    use std::ptr;
    use std::time::Duration;

    use gpui_kit::{App, AsyncApp, ForegroundExecutor};

    use super::InstanceGuard;

    const LOCK_FILE: &str = "instance.lock";
    const SHOW_NOTIFICATION: &[u8] = b"org.switchhostslite.show\0";
    const WAKE_ATTEMPTS: u32 = 20;
    const WAKE_INTERVAL: Duration = Duration::from_millis(50);

    type CFNotificationCenterRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFNotificationCallback = unsafe extern "C" fn(
        CFNotificationCenterRef,
        *const c_void,
        CFStringRef,
        *const c_void,
        *const c_void,
    );

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFNotificationCenterGetDistributedCenter() -> CFNotificationCenterRef;
        fn CFNotificationCenterAddObserver(
            center: CFNotificationCenterRef,
            observer: *const c_void,
            callback: CFNotificationCallback,
            name: CFStringRef,
            object: *const c_void,
            suspension_behavior: isize,
        );
        fn CFNotificationCenterRemoveObserver(
            center: CFNotificationCenterRef,
            observer: *const c_void,
            name: CFStringRef,
            object: *const c_void,
        );
        fn CFNotificationCenterPostNotification(
            center: CFNotificationCenterRef,
            name: CFStringRef,
            object: *const c_void,
            user_info: *const c_void,
            deliver_immediately: u8,
        );
        fn CFStringCreateWithCString(
            alloc: *const c_void,
            value: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(value: *const c_void);
    }

    struct Observer {
        app: AsyncApp,
        executor: ForegroundExecutor,
    }

    pub(super) fn acquire() -> io::Result<Option<InstanceGuard>> {
        let dir = crate::store::data_dir();
        std::fs::create_dir_all(&dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(dir.join(LOCK_FILE))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(InstanceGuard { _file: file })),
            Err(TryLockError::WouldBlock) => {
                wake_existing();
                Ok(None)
            }
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    fn notification_name() -> Option<CFStringRef> {
        // SAFETY: the name is NUL-terminated and static, and the returned
        // Core Foundation string is released by the caller.
        let name = unsafe {
            CFStringCreateWithCString(ptr::null(), SHOW_NOTIFICATION.as_ptr().cast(), 0x0800_0100)
        };
        (!name.is_null()).then_some(name)
    }

    fn wake_existing() {
        let Some(name) = notification_name() else {
            return;
        };
        // SAFETY: the distributed center is process-wide and never released;
        // the notification name stays alive through every post.
        unsafe {
            let center = CFNotificationCenterGetDistributedCenter();
            for _ in 0..WAKE_ATTEMPTS {
                CFNotificationCenterPostNotification(center, name, ptr::null(), ptr::null(), 1);
                std::thread::sleep(WAKE_INTERVAL);
            }
            CFRelease(name);
        }
    }

    pub(super) fn watch(cx: &mut App) {
        let Some(name) = notification_name() else {
            return;
        };
        // SAFETY: the distributed center is process-wide and never released.
        let center = unsafe { CFNotificationCenterGetDistributedCenter() };
        let observer = Box::into_raw(Box::new(Observer {
            app: cx.to_async(),
            executor: cx.foreground_executor().clone(),
        }));
        // SAFETY: the observer lives until on_app_quit unregisters it, and the
        // name stays alive until after removal. DeliverImmediately is enum 4.
        unsafe {
            CFNotificationCenterAddObserver(
                center,
                observer.cast(),
                on_show,
                name,
                ptr::null(),
                4,
            );
        }
        cx.on_app_quit(move |_| {
            // SAFETY: unregister before freeing the observer and notification
            // name; neither may be used by the notification center afterwards.
            unsafe {
                CFNotificationCenterRemoveObserver(center, observer.cast(), name, ptr::null());
                drop(Box::from_raw(observer));
                CFRelease(name);
            }
            async {}
        })
        .detach();
    }

    unsafe extern "C" fn on_show(
        _center: CFNotificationCenterRef,
        observer: *const c_void,
        _name: CFStringRef,
        _object: *const c_void,
        _user_info: *const c_void,
    ) {
        if observer.is_null() {
            return;
        }
        // SAFETY: watch keeps this allocation alive until it unregisters the
        // callback in on_app_quit.
        let observer = unsafe { &*(observer as *const Observer) };
        let app = observer.app.clone();
        observer
            .executor
            .spawn(async move {
                app.update(crate::tray::show);
            })
            .detach();
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::ptr;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, SetLastError,
    };
    use windows_sys::Win32::System::Threading::CreateMutexW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowExW, HWND_MESSAGE, PostMessageW, RegisterWindowMessageW,
    };

    use super::InstanceGuard;

    const MUTEX: &str = "Local\\SwitchHostsLite.Instance";
    pub(crate) const CLASS: &str = "SwitchHostsLiteTray";
    pub(crate) const TITLE: &str = "SwitchHosts Lite Tray";
    pub(crate) const SHOW_MESSAGE: &str = "SwitchHostsLite_Show";
    const WAKE_TIMEOUT: Duration = Duration::from_secs(5);
    const WAKE_INTERVAL: Duration = Duration::from_millis(50);

    impl Drop for InstanceGuard {
        fn drop(&mut self) {
            // SAFETY: this process owns the mutex handle until the guard drops.
            unsafe {
                CloseHandle(self._handle);
            }
        }
    }

    pub(super) fn acquire() -> io::Result<Option<InstanceGuard>> {
        let name = wide(MUTEX);
        // SAFETY: the name is NUL-terminated and outlives the call; no security
        // attributes or initial ownership are needed.
        unsafe {
            SetLastError(0);
            let handle = CreateMutexW(ptr::null(), 0, name.as_ptr());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                wake_existing();
                return Ok(None);
            }
            Ok(Some(InstanceGuard { _handle: handle }))
        }
    }

    fn wake_existing() {
        let class = wide(CLASS);
        let title = wide(TITLE);
        // SAFETY: the message name is NUL-terminated and outlives the call.
        let message = unsafe { RegisterWindowMessageW(wide(SHOW_MESSAGE).as_ptr()) };
        if message == 0 {
            return;
        }
        let deadline = Instant::now() + WAKE_TIMEOUT;
        loop {
            // SAFETY: both strings stay alive for the call, and the returned
            // handle is only passed back to PostMessageW.
            unsafe {
                let tray = FindWindowExW(HWND_MESSAGE, ptr::null_mut(), class.as_ptr(), title.as_ptr());
                if !tray.is_null() {
                    PostMessageW(tray, message, 0, 0);
                    return;
                }
            }
            if Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(WAKE_INTERVAL);
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }
}
