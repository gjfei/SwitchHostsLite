//! The platform glue the UI calls by name: the image picker on both backends
//! and the folder picker on macOS, implemented natively because gpui's path
//! prompt cannot filter by type.

#[cfg(target_os = "macos")]
pub use crate::macos::{prompt_for_directory, prompt_for_image};

#[cfg(windows)]
pub use crate::windows::prompt_for_image;
