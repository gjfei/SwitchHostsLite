use std::borrow::Cow;

use gpui_kit::App;
#[cfg(debug_assertions)]
use objc2::{AnyThread as _, MainThreadMarker};
#[cfg(debug_assertions)]
use objc2_app_kit::{NSApplication, NSImage};
#[cfg(debug_assertions)]
use objc2_foundation::NSData;

#[cfg(debug_assertions)]
const ICON_PNG: &[u8] = include_bytes!("../assets/icon.png");

/// The system monospace file. Loaded by bytes because Core Text's own
/// enumeration of this face is unusable to gpui on macOS 13: a zero trait
/// comes back as `CFBoolean`, gpui skips every face that is not all
/// `CFNumber`s, and the first editor layout then panics. The same bytes
/// carry real numbers, and registered fonts are searched first.
const MENLO_PATH: &str = "/System/Library/Fonts/Menlo.ttc";

pub fn register_mono_font(cx: &mut App) {
    let Ok(bytes) = std::fs::read(MENLO_PATH) else {
        return;
    };
    if let Err(err) = cx.text_system().add_fonts(vec![Cow::Owned(bytes)]) {
        eprintln!("register Menlo failed: {err:#}");
    }
}

/// Show the app icon in the Dock when running outside an `.app` bundle (e.g. `cargo run`).
#[cfg(debug_assertions)]
pub fn set_dock_icon() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(ICON_PNG);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: called on the main thread with a valid, retained image.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}
