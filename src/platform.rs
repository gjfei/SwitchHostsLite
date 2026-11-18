//! The one piece of platform glue the UI calls by name: the image picker,
//! which each backend implements itself because gpui's path prompt cannot
//! filter by file type.

#[cfg(target_os = "macos")]
pub use crate::macos::prompt_for_image;

#[cfg(windows)]
pub use crate::windows::prompt_for_image;
