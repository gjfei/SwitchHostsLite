//! The system tray icon and close-to-tray behaviour: closing the window hides
//! the app and, on macOS, drops its Dock tile so it stays in the menu bar only,
//! and only cmd-Q or the tray's Quit entry ends the process. The tray menu is
//! deliberately small: show the window, quit.
//!
//! Menu actions run on the main thread but outside any gpui callback, so they
//! reach the app through a foreground task holding an `AsyncApp` instead of a
//! direct borrow, which would panic while a callback is on the stack.

use gpui_kit::{App, Window};

pub fn init(window: &Window, cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        macos::init(cx);
    }
    #[cfg(windows)]
    windows::init(window, cx);
}

/// Bring the hidden app back as a normal Dock app, from the tray's Show entry
/// (and a Dock-icon click, which only applies when hiding left the icon up).
#[cfg(target_os = "macos")]
pub fn show(cx: &mut App) {
    macos::show(cx);
}

/// Drop the Dock tile at startup, for a login launch that opens no window.
#[cfg(target_os = "macos")]
pub fn enter_accessory() {
    macos::enter_accessory();
}

/// The `on_window_should_close` body: hide instead of closing.
pub fn hide(window: &Window, cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        cx.hide();
        macos::enter_accessory();
    }
    #[cfg(windows)]
    {
        let _ = cx;
        windows::hide(window);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;

    use gpui_kit::{App, AsyncApp, ForegroundExecutor};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, AnyThread as _, DefinedClass as _, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSImage, NSMenu, NSStatusBar, NSStatusItem,
        NSVariableStatusItemLength,
    };
    use objc2_foundation::{NSData, NSObjectProtocol, NSSize, NSString};
    use rust_i18n::t;

    const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");
    /// Menu-bar icons are drawn at about 18 pt; the source PNG is 256 px.
    const ICON_SIZE: f64 = 18.;

    struct Ivars {
        app: AsyncApp,
        executor: ForegroundExecutor,
    }

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and the ivars do
        // not implement Drop; MainThreadOnly keeps every access on the thread
        // the App lives on.
        #[unsafe(super = objc2::runtime::NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = Ivars]
        struct TrayTarget;

        unsafe impl NSObjectProtocol for TrayTarget {}

        impl TrayTarget {
            #[unsafe(method(showWindow:))]
            fn show_window(&self, _sender: Option<&AnyObject>) {
                let ivars = self.ivars();
                let app = ivars.app.clone();
                ivars.executor.spawn(async move {
                    app.update(show);
                })
                .detach();
            }

            #[unsafe(method(quitApp:))]
            fn quit_app(&self, _sender: Option<&AnyObject>) {
                let ivars = self.ivars();
                let app = ivars.app.clone();
                ivars.executor.spawn(async move {
                    app.update(|cx| cx.quit());
                })
                .detach();
            }
        }
    );

    impl TrayTarget {
        fn new(mtm: objc2::MainThreadMarker, ivars: Ivars) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ivars);
            // SAFETY: NSObject's init takes no arguments and returns self.
            unsafe { msg_send![super(this), init] }
        }
    }

    // NSMenuItem's target is weak, and the status bar item must outlive init;
    // both live until the process ends.
    thread_local! {
        static KEEP: RefCell<Option<(Retained<TrayTarget>, Retained<NSStatusItem>)>> =
            const { RefCell::new(None) };
    }

    pub fn init(cx: &mut App) {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let target = TrayTarget::new(mtm, Ivars { app: cx.to_async(), executor: cx.foreground_executor().clone() });

        let menu = NSMenu::new(mtm);
        // SAFETY: the selectors exist on TrayTarget, the strings are valid,
        // and the target outlives the menu through KEEP.
        unsafe {
            let show = menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(&t!("tray_show")),
                Some(sel!(showWindow:)),
                &NSString::from_str(""),
            );
            show.setTarget(Some(&target));
            let quit = menu.addItemWithTitle_action_keyEquivalent(
                &NSString::from_str(&t!("tray_quit")),
                Some(sel!(quitApp:)),
                &NSString::from_str(""),
            );
            quit.setTarget(Some(&target));
        }

        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            let data = NSData::with_bytes(ICON_PNG);
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                image.setSize(NSSize::new(ICON_SIZE, ICON_SIZE));
                button.setImage(Some(&image));
            }
        }
        item.setMenu(Some(&menu));
        KEEP.with(|keep| *keep.borrow_mut() = Some((target, item)));
    }

    pub fn show(cx: &mut App) {
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            // A hidden app is menu-bar only; a visible window is a normal Dock app.
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            app.unhide(None);
        }
        // A window created hidden at login was never ordered on-screen; bring it
        // front so the tray's Show entry makes it visible too.
        if let Some(handle) = cx.windows().into_iter().next() {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
        cx.activate(true);
    }

    /// Drop the Dock tile while the window is closed, leaving the app in the
    /// menu bar only.
    pub fn enter_accessory() {
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            NSApplication::sharedApplication(mtm)
                .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::cell::RefCell;

    use gpui_kit::{App, AsyncApp, ForegroundExecutor, Window};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use rust_i18n::t;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
        GetCursorPos, HWND_MESSAGE, IDI_APPLICATION, IsIconic, IsWindowVisible, LoadIconW,
        MF_STRING, RegisterClassW, RegisterWindowMessageW, SW_HIDE, SW_RESTORE, SW_SHOW,
        SetForegroundWindow, ShowWindow, TPM_RETURNCMD, TrackPopupMenu, WM_APP, WM_LBUTTONUP,
        WM_RBUTTONUP, WNDCLASSW,
    };

    /// The tray callback message; `lParam`'s low word carries the mouse event.
    const WM_TRAY: u32 = WM_APP + 1;
    const CLASS: &str = "SwitchHostsLiteTray";
    /// The installer posts this to ask a running instance to quit gracefully.
    /// Keep the name in sync with scripts/windows/installer.nsi.
    const QUIT_MESSAGE: &str = "SwitchHostsLite_Quit";
    /// The icon resource embedded by assets/app.rc.
    const ICON_RESOURCE: usize = 1;

    struct State {
        main: HWND,
        app: AsyncApp,
        executor: ForegroundExecutor,
        quit_msg: u32,
    }

    /// A copy of the state taken before acting, so the RefCell is never held
    /// across `TrackPopupMenu`'s nested message loop.
    struct Ctx {
        main: HWND,
        app: AsyncApp,
        executor: ForegroundExecutor,
    }

    enum Action {
        Toggle(HWND),
        Menu(HWND, Ctx),
        Quit(Ctx),
        Ignore,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
        static TRAY: RefCell<HWND> = const { RefCell::new(std::ptr::null_mut()) };
    }

    pub fn init(window: &Window, cx: &mut App) {
        let Some(main) = main_hwnd(window) else {
            return;
        };
        let class = wide(CLASS);
        let title = wide("SwitchHosts Lite Tray");
        // SAFETY: every wide string outlives the calls, the window class and
        // NOTIFYICONDATAW are zeroed before their named fields are set, and
        // the icon handle stays alive for the process (loaded from the exe).
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = instance;
            wc.lpszClassName = class.as_ptr();
            // A second instance reuses the registered class.
            RegisterClassW(&wc);
            let tray = CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if tray.is_null() {
                return;
            }
            let quit_msg = RegisterWindowMessageW(wide(QUIT_MESSAGE).as_ptr());
            let icon = LoadIconW(instance, ICON_RESOURCE as *const u16);
            let mut data: NOTIFYICONDATAW = std::mem::zeroed();
            data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = tray;
            data.uID = 1;
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = WM_TRAY;
            data.hIcon = if icon.is_null() { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) } else { icon };
            let tip = wide("SwitchHosts Lite");
            data.szTip[..tip.len()].copy_from_slice(&tip);
            if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
                DestroyWindow(tray);
                return;
            }
            STATE.with(|s| {
                *s.borrow_mut() = Some(State {
                    main,
                    app: cx.to_async(),
                    executor: cx.foreground_executor().clone(),
                    quit_msg,
                });
            });
            TRAY.with(|t| *t.borrow_mut() = tray);
        }
        cx.on_app_quit(|_| {
            remove_icon();
            async {}
        })
        .detach();
    }

    pub fn hide(window: &Window) {
        if let Some(main) = main_hwnd(window) {
            // SAFETY: the handle belongs to this process's main window.
            unsafe { ShowWindow(main, SW_HIDE) };
        }
    }

    fn main_hwnd(window: &Window) -> Option<HWND> {
        // The inherent Window::window_handle (gpui's own id) shadows the trait.
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return None;
        };
        Some(win32.hwnd.get() as HWND)
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let action = STATE.with(|s| {
            let state = s.borrow();
            let state = state.as_ref()?;
            if msg == WM_TRAY {
                return Some(match lparam as u32 & 0xffff {
                    WM_LBUTTONUP => Action::Toggle(state.main),
                    WM_RBUTTONUP => Action::Menu(state.main, state.ctx()),
                    _ => Action::Ignore,
                });
            }
            (msg != 0 && msg == state.quit_msg).then(|| Action::Quit(state.ctx()))
        });
        match action {
            Some(Action::Toggle(main)) => toggle(main),
            Some(Action::Menu(tray, ctx)) => show_menu(tray, &ctx),
            Some(Action::Quit(ctx)) => quit(&ctx),
            Some(Action::Ignore) => {}
            // SAFETY: unhandled messages go to the default procedure.
            None => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
        0
    }

    impl State {
        fn ctx(&self) -> Ctx {
            Ctx { main: self.main, app: self.app.clone(), executor: self.executor.clone() }
        }
    }

    fn toggle(main: HWND) {
        // SAFETY: the handle belongs to this process's main window.
        unsafe {
            if IsWindowVisible(main) != 0 && IsIconic(main) == 0 {
                ShowWindow(main, SW_HIDE);
            } else {
                show_main(main);
            }
        }
    }

    fn show_main(main: HWND) {
        // SAFETY: the handle belongs to this process's main window.
        unsafe {
            ShowWindow(main, if IsIconic(main) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(main);
        }
    }

    fn show_menu(tray: HWND, ctx: &Ctx) {
        let mut point = POINT { x: 0, y: 0 };
        let show = wide(&t!("tray_show"));
        let quit_label = wide(&t!("tray_quit"));
        // SAFETY: the menu lives until DestroyMenu, the labels outlive the
        // calls, and the cursor position is read before the menu opens.
        unsafe {
            GetCursorPos(&mut point);
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return;
            }
            AppendMenuW(menu, MF_STRING, 1, show.as_ptr());
            AppendMenuW(menu, MF_STRING, 2, quit_label.as_ptr());
            // Without this the menu does not dismiss on a click outside it.
            SetForegroundWindow(tray);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD, point.x, point.y, 0, tray, std::ptr::null());
            DestroyMenu(menu);
            match cmd {
                1 => show_main(ctx.main),
                2 => quit(ctx),
                _ => {}
            }
        }
    }

    fn quit(ctx: &Ctx) {
        let app = ctx.app.clone();
        ctx.executor
            .spawn(async move {
                app.update(|cx| cx.quit());
            })
            .detach();
    }

    fn remove_icon() {
        TRAY.with(|t| {
            let tray = *t.borrow();
            if tray.is_null() {
                return;
            }
            // SAFETY: the data names the icon added in init.
            unsafe {
                let mut data: NOTIFYICONDATAW = std::mem::zeroed();
                data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                data.hWnd = tray;
                data.uID = 1;
                Shell_NotifyIconW(NIM_DELETE, &data);
            }
        });
    }
}