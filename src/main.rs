#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod autostart;
mod browser;
mod dock;
mod editor_ops;
mod highlight;
mod hosts;
mod http;
mod i18n;
#[cfg(target_os = "macos")]
mod macos;
mod panels;
mod platform;
mod remote;
mod settings;
mod store;
mod switchhosts;
mod themes;
mod tile;
#[cfg(any(target_os = "macos", windows))]
mod tray;
mod updater;
#[cfg(windows)]
mod winicon;
#[cfg(windows)]
mod windows;

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
#[cfg(target_os = "macos")]
use gpui_kit::component::TITLE_BAR_HEIGHT;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

actions!(switch_hosts, [Quit]);

rust_i18n::i18n!("locales", fallback = "en");

// Icons used by the app that are not in the default component bundle. The
// default bundle is a 106-icon subset, and `Icon` draws nothing for a path the
// asset source does not serve, so every icon the app names must appear here
// (or already be in that subset, like the `Close` cross used for clearing).
icon_assets!(ExtraIcons, [
    Clock, Globe, Layers, List, Lock, Monitor, Pencil, PencilLine, Play, RefreshCw, RotateCcw, Save, Square, Trash
]);

// The bundle icons (icns/ico) are not assets gpui can load; the About dialog
// shows this copy.
pub(crate) const ABOUT_ICON: &str = "icons/app-icon.png";

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == ABOUT_ICON {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/icon.png"))));
        }
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        if ABOUT_ICON.starts_with(path) {
            paths.push(ABOUT_ICON.into());
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

fn main() {
    // The helpers that `hosts::write_elevated` and the updater start do their
    // work and exit; they must not start the app.
    #[cfg(windows)]
    {
        hosts::run_as_elevated_helper();
        updater::run_update_installer_helper();
    }

    let application = gpui_kit::application().with_assets(AppAssets);
    // A hidden app is restored as a normal Dock app from the tray menu.
    #[cfg(target_os = "macos")]
    application.on_reopen(tray::show);
    application.run(|cx| {
            // Default on macOS is to keep running with no windows, which leaves the Dock icon.
            #[cfg(target_os = "macos")]
            cx.set_quit_mode(QuitMode::LastWindowClosed);
            // Before any text is laid out; see `register_mono_font`.
            #[cfg(target_os = "macos")]
            macos::register_mono_font(cx);
            gpui_kit::init(cx);
            themes::load(cx);
            app::init(cx);
            #[cfg(all(target_os = "macos", debug_assertions))]
            macos::set_dock_icon();
            app::bind_keys(cx);
            browser::clean_stale(&std::env::temp_dir());
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            #[cfg(not(target_os = "macos"))]
            cx.bind_keys([KeyBinding::new("ctrl-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(1000.), px(680.)), cx);
            let titlebar = TitlebarOptions {
                title: Some("SwitchHosts Lite".into()),
                // The kit's default offset centres the traffic lights in its
                // own `TITLE_BAR_HEIGHT` bar. This window renders a taller top
                // bar, so push the lights down by half the difference to keep
                // them on the bar's centre line instead of hugging its top.
                #[cfg(target_os = "macos")]
                traffic_light_position: Some(point(
                    px(9.),
                    px(9.) + (app::TOP_BAR_HEIGHT - TITLE_BAR_HEIGHT) * 0.5,
                )),
                ..TitleBar::title_bar_options()
            };
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                titlebar: Some(titlebar),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                // Closing hides to the tray instead of quitting; the tray's
                // Quit entry (or cmd-Q) ends the process and flushes saves
                // through `on_app_quit`.
                #[cfg(any(target_os = "macos", windows))]
                window.on_window_should_close(cx, |window, cx| {
                    tray::hide(window, cx);
                    false
                });
                #[cfg(any(target_os = "macos", windows))]
                tray::init(window, cx);
                cx.new(|cx| app::SwitchHostsApp::new(window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
            updater::init(cx);
        });
}
