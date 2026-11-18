use std::borrow::Cow;
use std::path::PathBuf;

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

/// An open panel limited to image files, resolving to the picked path.
///
/// gpui's own path prompt cannot filter by type, so this builds the panel the
/// same way its macOS backend does (app-modal, block-based completion) and adds
/// `allowedContentTypes` = `public.image`, the umbrella type every format
/// `NSImage` decodes conforms to. `None` when cancelled or off the main thread.
pub fn prompt_for_image(prompt: &str) -> impl Future<Output = Option<PathBuf>> + use<> {
    use std::cell::Cell;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::{msg_send, MainThreadMarker};
    use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel};
    use objc2_foundation::{NSArray, NSString};
    use objc2_uniform_type_identifiers::UTType;

    let (tx, rx) = futures_channel::oneshot::channel();
    let sender = Cell::new(Some(tx));

    if MainThreadMarker::new().is_some() {
        // SAFETY: on the main thread; `openPanel` is typed non-null but AppKit
        // can return nil when it cannot create the panel, as gpui's own
        // backend notes.
        let panel: Option<Retained<NSOpenPanel>> =
            unsafe { msg_send![objc2::class!(NSOpenPanel), openPanel] };
        if let Some(panel) = panel {
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            panel.setCanCreateDirectories(false);
            panel.setResolvesAliases(false);
            panel.setPrompt(Some(&NSString::from_str(prompt)));
            if let Some(image) = UTType::typeWithIdentifier(&NSString::from_str("public.image")) {
                let types = NSArray::from_retained_slice(&[image]);
                panel.setAllowedContentTypes(&types);
            }
            let handler = RcBlock::new({
                let panel = panel.clone();
                move |response: NSModalResponse| {
                    let Some(tx) = sender.take() else {
                        return;
                    };
                    let picked = (response == NSModalResponseOK)
                        .then(|| panel.URLs().iter().next().and_then(|url| url.to_file_path()))
                        .flatten();
                    let _ = tx.send(picked);
                }
            });
            panel.beginWithCompletionHandler(&handler);
        }
    }

    async move { rx.await.ok().flatten() }
}
