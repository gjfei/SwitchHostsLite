#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod editor_ops;
mod highlight;
mod hosts;
mod i18n;
#[cfg(target_os = "macos")]
mod macos;
mod settings;
mod store;
mod switchhosts;
mod themes;
mod updater;

use std::borrow::Cow;

use gpui_kit::assets::{Assets, icon_assets};
use gpui_kit::component::TitleBar;
use gpui_kit::*;

actions!(switch_hosts, [Quit]);

rust_i18n::i18n!("locales", fallback = "en");

// Icons used by the app that are not in the default component bundle.
icon_assets!(ExtraIcons, [Clock, Layers, List, Lock, Monitor, Pencil, RotateCcw, Save, Trash]);

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

fn main() {
    // The elevated child that `hosts::write_elevated` starts only writes the
    // staged hosts, then exits; it must not start the app.
    #[cfg(windows)]
    hosts::run_as_elevated_helper();

    gpui_kit::application()
        .with_assets(AppAssets)
        .run(|cx| {
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
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("SwitchHosts Lite".into()),
                    ..TitleBar::title_bar_options()
                }),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                // The close button destroys the NSWindow immediately. GPUI only
                // drops its window on a later turn, and until something calls
                // quit the process stays in the Dock. `cx.quit` still flushes
                // pending saves through `on_app_quit`.
                #[cfg(target_os = "macos")]
                window.on_window_should_close(cx, |_, cx| {
                    cx.quit();
                    true
                });
                cx.new(|cx| app::SwitchHostsApp::new(window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
            updater::init(cx);
        });
}
