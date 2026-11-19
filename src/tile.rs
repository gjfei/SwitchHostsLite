//! Cached profile pictures, composited once so drawing them is just a file read.
//!
//! One picture per profile, used both by the app UI and by the Dock / taskbar
//! pipe: the user's own image as-is, or the browser's app icon with a letter
//! badge whose background is the profile color (neutral grey while no color is
//! set). A custom image never carries a badge, so the color never touches it.

use std::path::PathBuf;

use gpui_kit::{App, ImageSource};

use crate::{browser::BrowserProfile, store};

pub fn tile_path(id: &str) -> PathBuf {
    store::data_dir().join("profile-tiles").join(format!("{id}.png"))
}

/// The badge-less icon the previous layout cached next to the mark; deleting it
/// on sight keeps the cache directory from carrying two pictures per profile.
fn legacy_icon_path(id: &str) -> PathBuf {
    store::data_dir()
        .join("profile-tiles")
        .join(format!("{id}-icon.png"))
}

/// The user's picture, normalized to a 256px PNG by [`save_image`].
pub fn image_path(id: &str) -> PathBuf {
    store::data_dir().join("profile-images").join(format!("{id}.png"))
}

pub fn remove(id: &str) {
    let _ = std::fs::remove_file(tile_path(id));
    let _ = std::fs::remove_file(legacy_icon_path(id));
}

pub fn remove_image(id: &str) {
    let _ = std::fs::remove_file(image_path(id));
}

/// gpui caches decoded images by path alone, so the info panel preview keeps
/// showing the old picture until its asset is dropped after a recomposite.
pub fn invalidate_preview(id: &str, cx: &mut App) {
    ImageSource::from(tile_path(id)).remove_asset(cx);
}

/// The badge letter: the name's first character, uppercased where that applies.
#[cfg(any(target_os = "macos", windows, test))]
pub fn badge_char(name: &str) -> Option<char> {
    name.trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().next().unwrap_or(c))
}

/// A neutral badge background, used while the profile carries no color, so the
/// letter stays readable on any app icon.
#[cfg(any(target_os = "macos", windows, test))]
const NEUTRAL_BADGE: [u8; 3] = [120, 124, 133];

/// What the badge is filled with: the profile color when the user picked one,
/// a neutral grey otherwise. The badge itself is always drawn — only a custom
/// picture replaces it. The default mode is the everyday browser, so its color
/// is ignored here exactly as it is for the window mark.
#[cfg(any(target_os = "macos", windows, test))]
fn badge_accent(profile: &BrowserProfile) -> [u8; 3] {
    crate::browser::mark_color(profile).unwrap_or(NEUTRAL_BADGE)
}

/// The cached picture, compositing it first when missing. `None` where no
/// platform backend can resolve an executable's icon.
pub fn ensure(profile: &BrowserProfile) -> Option<PathBuf> {
    #[cfg(any(target_os = "macos", windows))]
    {
        let path = tile_path(&profile.id);
        if path.exists() {
            return Some(path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        compose(profile, badge_accent(profile), &path).map(|()| path)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = profile;
        None
    }
}

/// Recomposite after a color, name, picture or path change; the picture exists
/// for every profile now, only its badge background follows the color.
pub fn refresh(profile: &BrowserProfile) {
    #[cfg(any(target_os = "macos", windows))]
    {
        let path = tile_path(&profile.id);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = compose(profile, badge_accent(profile), &path);
        let _ = std::fs::remove_file(legacy_icon_path(&profile.id));
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = profile;
    }
}

/// Normalize a user picture into the cached 256px PNG, aspect-fit on
/// transparency. AppKit loading, so this must run on the main thread.
/// `None` when the file is not a readable image.
#[cfg(target_os = "macos")]
pub fn save_image(id: &str, source: &std::path::Path) -> Option<()> {
    use objc2::AnyThread as _;
    use objc2_app_kit::{NSCompositingOperation, NSImage};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    let path = image_path(id);
    let source = NSString::from_str(&source.to_string_lossy());
    // Load and validate outside draw_png: an empty draw would still encode a
    // blank PNG, which must not count as a successful import.
    let img = NSImage::initWithContentsOfFile(NSImage::alloc(), &source)?;
    let size = img.size();
    if size.width <= 0.0 || size.height <= 0.0 {
        return None;
    }
    let png = draw_png(|| {
        let side = 256.0;
        let scale = (side / size.width).min(side / size.height);
        let (w, h) = (size.width * scale, size.height * scale);
        let rect = NSRect::new(
            NSPoint::new((side - w) / 2.0, (side - h) / 2.0),
            NSSize::new(w, h),
        );
        img.drawInRect_fromRect_operation_fraction(
            rect,
            NSRect::ZERO,
            NSCompositingOperation::SourceOver,
            1.0,
        );
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::write(path, png).ok()
}

/// Normalize a user picture into the cached 256px PNG, aspect-fit on
/// transparency. Pure Rust, so it may run on any thread.
/// `None` when the file is not a readable image.
#[cfg(windows)]
pub fn save_image(id: &str, source: &std::path::Path) -> Option<()> {
    let image = image::open(source).ok()?;
    if image.width() == 0 || image.height() == 0 {
        return None;
    }
    let canvas = crate::winicon::fit(&image.to_rgba8());
    crate::winicon::write_png(&image_path(id), &canvas)
}

/// The user's picture as-is (no badge), else the browser icon with a letter
/// badge filled with `accent`.
#[cfg(target_os = "macos")]
fn compose(profile: &BrowserProfile, accent: [u8; 3], path: &std::path::Path) -> Option<()> {
    use objc2::AnyThread as _;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSAttributedStringNSStringDrawing as _, NSBezierPath, NSColor, NSCompositingOperation,
        NSFont, NSFontAttributeName, NSForegroundColorAttributeName, NSImage,
        NSImageNameApplicationIcon, NSWorkspace,
    };
    use objc2_foundation::{
        NSAttributedString, NSAttributedStringKey, NSDictionary, NSPoint, NSRect, NSSize, NSString,
    };

    let custom = profile
        .custom_image
        .then(|| image_path(&profile.id))
        .filter(|p| p.exists());
    let exe = profile.display_executable();
    // iconForFile on the inner executable yields the generic "exec" icon;
    // the bundle carries the real app icon.
    let icon_path = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .unwrap_or(&exe);
    let icon_file = NSString::from_str(&icon_path.to_string_lossy());
    let letter = badge_char(&profile.name).map(|c| c.to_string());
    let png = draw_png(|| {
        // SAFETY: main-thread AppKit drawing inside the bitmap context.
        unsafe {
            let full = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(256.0, 256.0));
            if let Some(file) = &custom
                && let Some(img) =
                    NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(&file.to_string_lossy()))
            {
                img.drawInRect_fromRect_operation_fraction(
                    full,
                    NSRect::ZERO,
                    NSCompositingOperation::SourceOver,
                    1.0,
                );
                return;
            }
            let icon = if icon_path.exists() {
                NSWorkspace::sharedWorkspace().iconForFile(&icon_file)
            } else {
                let Some(icon) = NSImage::imageNamed(NSImageNameApplicationIcon) else {
                    return;
                };
                icon
            };
            icon.drawInRect_fromRect_operation_fraction(
                full,
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
                1.0,
            );
            // No custom picture, so the badge always rides on the app icon.
            // A white ring keeps it readable on any icon colors.
            let outer = NSRect::new(NSPoint::new(126.0, 6.0), NSSize::new(124.0, 124.0));
            NSColor::whiteColor().setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(outer, 30.0, 30.0).fill();
            let inner = NSRect::new(NSPoint::new(132.0, 12.0), NSSize::new(112.0, 112.0));
            let [r, g, b] = accent;
            NSColor::colorWithSRGBRed_green_blue_alpha(
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
                1.0,
            )
            .setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(inner, 26.0, 26.0).fill();
            if let Some(letter) = &letter {
                let font = NSFont::boldSystemFontOfSize(64.0);
                let white = NSColor::whiteColor();
                let attrs = NSDictionary::<NSAttributedStringKey, AnyObject>::from_slices(
                    &[NSFontAttributeName, NSForegroundColorAttributeName],
                    &[&font as &AnyObject, &white as &AnyObject],
                );
                let text = NSAttributedString::initWithString_attributes(
                    NSAttributedString::alloc(),
                    &NSString::from_str(letter),
                    Some(&attrs),
                );
                let size = text.size();
                text.drawAtPoint(NSPoint::new(
                    inner.origin.x + (inner.size.width - size.width) / 2.0,
                    inner.origin.y + (inner.size.height - size.height) / 2.0,
                ));
            }
        }
    })?;
    std::fs::write(path, png).ok()
}

/// The same picture, drawn with GDI and the `image` crate instead of AppKit:
/// the executable's own icon resource plus the letter badge.
#[cfg(windows)]
fn compose(profile: &BrowserProfile, accent: [u8; 3], path: &std::path::Path) -> Option<()> {
    crate::winicon::compose(profile, accent, path)
}

/// Render into an offscreen 256px bitmap and encode it as PNG. AppKit drawing,
/// so this must run on the main thread.
#[cfg(target_os = "macos")]
fn draw_png(draw: impl FnOnce()) -> Option<Vec<u8>> {
    use objc2::AnyThread as _;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext,
    };
    use objc2_foundation::NSDictionary;

    // SAFETY: the bitmap context is current only between save/restore, and
    // every object outlives its use.
    unsafe {
        let side = 256isize;
        let rep = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            side * 4,
            32,
        )?;
        let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&ctx));
        draw();
        NSGraphicsContext::restoreGraphicsState_class();
        let data = rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())?;
        Some(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_path_is_per_profile_under_data_dir() {
        let path = tile_path("p1");
        assert!(path.ends_with("profile-tiles/p1.png"));
        assert!(path.starts_with(store::data_dir()));
    }

    #[test]
    fn badge_accent_prefers_the_profile_color_and_falls_back_to_neutral() {
        let mut profile = BrowserProfile::new("p1");
        assert_eq!(badge_accent(&profile), NEUTRAL_BADGE);
        profile.tile_color = Some([1, 2, 3]);
        assert_eq!(badge_accent(&profile), [1, 2, 3]);
        // The default mode is the everyday browser: its color is not a mark.
        profile.mode = crate::browser::ProfileMode::Default;
        assert_eq!(badge_accent(&profile), NEUTRAL_BADGE);
    }

    #[test]
    fn image_path_is_per_profile_under_data_dir() {
        let path = image_path("p1");
        assert!(path.ends_with("profile-images/p1.png"));
        assert!(path.starts_with(store::data_dir()));
    }

    #[test]
    fn badge_char_takes_the_first_letter() {
        assert_eq!(badge_char("Work"), Some('W'));
        assert_eq!(badge_char("work"), Some('W'));
        assert_eq!(badge_char(" 工作 "), Some('工'));
        assert_eq!(badge_char("ßeta"), Some('S'));
        assert_eq!(badge_char("   "), None);
        assert_eq!(badge_char(""), None);
    }
}