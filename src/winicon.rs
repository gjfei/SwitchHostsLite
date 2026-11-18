//! Windows profile pictures: the browser's own icon plus the first-letter
//! badge, composited into the same 256 px PNG the macOS build draws with
//! AppKit.
//!
//! The icon comes from the executable's `RT_GROUP_ICON` resource, which carries
//! a 256 px PNG for every modern browser; `image` decodes that entry, so no
//! 32 px `HICON` has to be upscaled. The letter is the one part that needs a
//! font, so GDI draws it white on black into a small DIB and those grey levels
//! are used as coverage: GDI never touches our alpha, no ClearType fringes end
//! up in the picture, and CJK names work through GDI's font linking.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use image::{Rgba, RgbaImage, imageops::FilterType};
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC,
    DeleteObject, DrawTextW, FW_BOLD, FillRect, GetDC, HGDIOBJ, OUT_DEFAULT_PRECIS, OPAQUE,
    ReleaseDC, SelectObject, SetBkColor, SetBkMode, SetTextColor,
};
use windows_sys::Win32::System::LibraryLoader::{
    EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LoadLibraryExW, LoadResource,
    LockResource, SizeofResource,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{RT_GROUP_ICON, RT_ICON};

use crate::browser::BrowserProfile;

/// The cached picture's side, matching the macOS composite.
const SIDE: u32 = 256;
/// The white ring around the badge, in image coordinates (y grows downwards,
/// so this is the bottom-right corner exactly like the AppKit drawing).
const RING: (f32, f32, f32, f32) = (126., 126., 250., 250.);
const RING_RADIUS: f32 = 30.;
/// The accent disc inset by 6 px inside the ring.
const BADGE: (f32, f32, f32, f32) = (132., 132., 244., 244.);
const BADGE_RADIUS: f32 = 26.;
/// The square the letter is rasterized into, and the font size it is drawn at.
const GLYPH: i32 = 112;
const GLYPH_SIZE: i32 = 64;

/// Compose the cached picture and write it to `path`. `None` when the browser's
/// icon cannot be read, which leaves the globe placeholder in place.
pub fn compose(profile: &BrowserProfile, accent: [u8; 3], path: &Path) -> Option<()> {
    let custom = profile
        .custom_image
        .then(|| crate::tile::image_path(&profile.id))
        .filter(|path| path.exists());

    let canvas = match custom {
        // A custom picture replaces the icon and carries no badge, so the color
        // never touches it.
        Some(path) => fit(&image::open(path).ok()?.to_rgba8()),
        None => {
            let mut canvas = fit(&exe_icon(&profile.display_executable())?);
            draw_badge(&mut canvas, crate::tile::badge_char(&profile.name), accent);
            canvas
        }
    };
    write_png(path, &canvas)
}

/// Encode an RGBA canvas as PNG on disk, creating its directory.
pub(crate) fn write_png(path: &Path, canvas: &RgbaImage) -> Option<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let file = std::fs::File::create(path).ok()?;
    image::DynamicImage::ImageRgba8(canvas.clone())
        .write_to(std::io::BufWriter::new(file), image::ImageFormat::Png)
        .ok()
}

/// Scale a picture to fit the 256 px square, centred on transparency.
pub(crate) fn fit(source: &RgbaImage) -> RgbaImage {
    let (width, height) = source.dimensions();
    if width == 0 || height == 0 {
        return RgbaImage::from_pixel(SIDE, SIDE, Rgba([0, 0, 0, 0]));
    }
    let scale = (SIDE as f32 / width as f32).min(SIDE as f32 / height as f32);
    let scaled_width = ((width as f32 * scale).round() as u32).clamp(1, SIDE);
    let scaled_height = ((height as f32 * scale).round() as u32).clamp(1, SIDE);
    let resized = image::imageops::resize(source, scaled_width, scaled_height, FilterType::Lanczos3);
    let mut canvas = RgbaImage::from_pixel(SIDE, SIDE, Rgba([0, 0, 0, 0]));
    image::imageops::overlay(
        &mut canvas,
        &resized,
        i64::from((SIDE - scaled_width) / 2),
        i64::from((SIDE - scaled_height) / 2),
    );
    canvas
}

/// The executable's icon: its largest `RT_GROUP_ICON` entry.
fn exe_icon(exe: &Path) -> Option<RgbaImage> {
    let bytes = icon_resource(exe)?;
    // Modern icons store the 256 px image as PNG; older ones store a DIB,
    // which `icon_group_entry` already wrapped in an ICO container.
    let decoded = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        image::load_from_memory(&bytes)
    } else {
        image::load_from_memory_with_format(&bytes, image::ImageFormat::Ico)
    };
    Some(decoded.ok()?.to_rgba8())
}

/// Wrap raw icon image data in a one-entry ICO container.
fn ico(data: &[u8], width: u8, height: u8, bpp: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(22 + data.len());
    out.extend_from_slice(&[0, 0, 1, 0]); // reserved, type: icon
    out.extend_from_slice(&1u16.to_le_bytes()); // one entry
    out.push(width); // 0 means 256
    out.push(height);
    out.push(0); // palette size
    out.push(0); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
    out.extend_from_slice(&bpp.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes()); // image offset
    out.extend_from_slice(data);
    out
}

/// Read the largest image of the executable's first icon group.
fn icon_resource(exe: &Path) -> Option<Vec<u8>> {
    let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: the wide path is NUL-terminated and outlives the call; loading as
    // a data file maps the module's resources without running any of its code.
    unsafe {
        let module = LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_AS_DATAFILE);
        if module.is_null() {
            return None;
        }
        let data = icon_group_entry(module);
        FreeLibrary(module);
        data
    }
}

/// The best entry of the lowest-numbered icon group: largest area, then bit
/// depth, so a 256 px PNG wins over the 16 px one.
unsafe fn icon_group_entry(module: HMODULE) -> Option<Vec<u8>> {
    let mut ids: Vec<u16> = Vec::new();
    // SAFETY: the callback only writes to the `Vec` passed through `lparam`.
    unsafe {
        EnumResourceNamesW(
            module,
            RT_GROUP_ICON,
            Some(collect_group_id),
            &mut ids as *mut Vec<u16> as isize,
        );
    }
    let group_id = ids.into_iter().min()?;

    let group = unsafe { resource(module, group_id, RT_GROUP_ICON) }?;
    if group.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes([group[4], group[5]]) as usize;
    let mut best: Option<(u32, u16, u8, u8, u16)> = None;
    for index in 0..count {
        let at = 6 + index * 14;
        let Some(entry) = group.get(at..at + 14) else {
            break;
        };
        let width = entry[0];
        let height = entry[1];
        let depth = u16::from_le_bytes([entry[6], entry[7]]);
        let id = u16::from_le_bytes([entry[12], entry[13]]);
        let area = u32::from(width.max(1)) * u32::from(height.max(1));
        // A width or height of 0 means 256, the largest an ICO can describe.
        let area = if width == 0 && height == 0 { 256 * 256 } else { area };
        if best.is_none_or(|(best_area, best_depth, ..)| (area, depth) > (best_area, best_depth)) {
            best = Some((area, depth, width, height, id));
        }
    }
    let (_, depth, width, height, id) = best?;
    let data = unsafe { resource(module, id, RT_ICON) }?;
    // The ICO header needs the real geometry; a PNG entry ignores the wrapper,
    // but a DIB entry is sized from it.
    Some(if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        data
    } else {
        ico(&data, width, height, depth)
    })
}

/// `EnumResourceNamesW` callback: keep the integer ids, which are the ones an
/// icon group can be found by.
unsafe extern "system" fn collect_group_id(
    _module: HMODULE,
    _kind: windows_sys::core::PCWSTR,
    name: windows_sys::core::PCWSTR,
    lparam: isize,
) -> BOOL {
    let ids = unsafe { &mut *(lparam as *mut Vec<u16>) };
    let value = name as usize;
    // String-named resources carry a pointer above the 16-bit range.
    if value >> 16 == 0 {
        ids.push(value as u16);
    }
    1
}

/// Copy a resource's bytes out of the mapped module.
unsafe fn resource(module: HMODULE, id: u16, kind: windows_sys::core::PCWSTR) -> Option<Vec<u8>> {
    // SAFETY: `id` is passed as a MAKEINTRESOURCE-style integer, and the handle
    // is only locked for the duration of the copy.
    unsafe {
        let found = FindResourceW(module, id as usize as *const u16, kind);
        if found.is_null() {
            return None;
        }
        let size = SizeofResource(module, found);
        if size == 0 {
            return None;
        }
        let loaded = LoadResource(module, found);
        if loaded.is_null() {
            return None;
        }
        let data = LockResource(loaded);
        if data.is_null() {
            return None;
        }
        Some(std::slice::from_raw_parts(data as *const u8, size as usize).to_vec())
    }
}

/// The white ring, the accent disc and the letter on top of it.
fn draw_badge(canvas: &mut RgbaImage, letter: Option<char>, accent: [u8; 3]) {
    fill_round_rect(canvas, RING, RING_RADIUS, [255, 255, 255, 255]);
    fill_round_rect(canvas, BADGE, BADGE_RADIUS, [accent[0], accent[1], accent[2], 255]);
    let Some(letter) = letter else {
        return;
    };
    let Some(coverage) = glyph(letter) else {
        return;
    };
    for (index, alpha) in coverage.iter().enumerate() {
        if *alpha == 0 {
            continue;
        }
        let x = BADGE.0 as u32 + index as u32 % GLYPH as u32;
        let y = BADGE.1 as u32 + index as u32 / GLYPH as u32;
        blend(canvas, x, y, [255, 255, 255, 255], f32::from(*alpha) / 255.0);
    }
}

/// Fill a rounded rectangle with 3×3 supersampling, so its edges do not look
/// like stairs once the 256 px picture is drawn at 48 px.
fn fill_round_rect(canvas: &mut RgbaImage, rect: (f32, f32, f32, f32), radius: f32, color: [u8; 4]) {
    let (left, top, right, bottom) = rect;
    let xs = left.floor().max(0.0) as u32;
    let ys = top.floor().max(0.0) as u32;
    let xe = (right.ceil() as u32).min(canvas.width());
    let ye = (bottom.ceil() as u32).min(canvas.height());
    const STEPS: u32 = 3;
    for y in ys..ye {
        for x in xs..xe {
            let mut hits = 0;
            for sy in 0..STEPS {
                for sx in 0..STEPS {
                    let px = x as f32 + (sx as f32 + 0.5) / STEPS as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / STEPS as f32;
                    let cx = px.clamp(left + radius, right - radius);
                    let cy = py.clamp(top + radius, bottom - radius);
                    let (dx, dy) = (px - cx, py - cy);
                    if dx * dx + dy * dy <= radius * radius && px <= right && py <= bottom {
                        hits += 1;
                    }
                }
            }
            if hits > 0 {
                let volume = hits as f32 / (STEPS * STEPS) as f32;
                blend(canvas, x, y, color, volume);
            }
        }
    }
}

/// Source-over blend of one pixel.
fn blend(canvas: &mut RgbaImage, x: u32, y: u32, color: [u8; 4], alpha: f32) {
    if x >= canvas.width() || y >= canvas.height() {
        return;
    }
    let source_alpha = alpha.clamp(0.0, 1.0);
    let pixel = canvas.get_pixel_mut(x, y);
    let dest_alpha = f32::from(pixel.0[3]) / 255.0;
    let out_alpha = source_alpha + dest_alpha * (1.0 - source_alpha);
    if out_alpha <= 0.0 {
        return;
    }
    for (channel, source) in pixel.0.iter_mut().zip(color).take(3) {
        let source = f32::from(source) / 255.0;
        let dest = f32::from(*channel) / 255.0;
        let value = (source * source_alpha + dest * dest_alpha * (1.0 - source_alpha)) / out_alpha;
        *channel = (value * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    pixel.0[3] = (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// The letter as 8-bit coverage: GDI draws it white on black, so a pixel's
/// brightness is how much of it the glyph covers.
fn glyph(letter: char) -> Option<Vec<u8>> {
    let face: Vec<u16> = if letter.is_ascii() {
        "Segoe UI"
    } else {
        "Microsoft YaHei"
    }
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let text: Vec<u16> = letter.to_string().encode_utf16().collect();
    let side = GLYPH;
    let mut coverage = vec![0u8; (side * side) as usize];

    // SAFETY: every GDI object is created here and released before returning;
    // `bits` points into the DIB section, which outlives every read of it.
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let dc = CreateCompatibleDC(screen);
        if !screen.is_null() {
            ReleaseDC(std::ptr::null_mut(), screen);
        }
        if dc.is_null() {
            return None;
        }

        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = side;
        // Negative height: top-down rows, so the buffer is in reading order.
        info.bmiHeader.biHeight = -side;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return None;
        }
        let old_bitmap = SelectObject(dc, bitmap as HGDIOBJ);

        let black = CreateSolidBrush(0);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: side,
            bottom: side,
        };
        FillRect(dc, &rect, black);
        DeleteObject(black as HGDIOBJ);

        let font = CreateFontW(
            -GLYPH_SIZE,
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            face.as_ptr(),
        );
        let old_font = SelectObject(dc, font as HGDIOBJ);
        SetTextColor(dc, 0x00FF_FFFF);
        SetBkColor(dc, 0);
        SetBkMode(dc, OPAQUE as i32);
        DrawTextW(
            dc,
            text.as_ptr(),
            text.len() as i32,
            &mut rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );

        let pixels = std::slice::from_raw_parts(bits as *const u8, (side * side * 4) as usize);
        for (index, slot) in coverage.iter_mut().enumerate() {
            let at = index * 4;
            // BGRA, white on black: the brightest channel is the coverage.
            *slot = pixels[at].max(pixels[at + 1]).max(pixels[at + 2]);
        }

        SelectObject(dc, old_font);
        SelectObject(dc, old_bitmap);
        DeleteObject(font as HGDIOBJ);
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(dc);
    }
    Some(coverage)
}

#[cfg(test)]
mod tests {
    #[test]
    fn ico_header_describes_one_entry() {
        let wrapped = super::ico(&[1, 2, 3, 4], 32, 32, 32);
        assert_eq!(wrapped.len(), 22 + 4);
        assert_eq!(&wrapped[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([wrapped[4], wrapped[5]]), 1);
        assert_eq!(u32::from_le_bytes([wrapped[14], wrapped[15], wrapped[16], wrapped[17]]), 4);
        assert_eq!(u32::from_le_bytes([wrapped[18], wrapped[19], wrapped[20], wrapped[21]]), 22);
    }
}
