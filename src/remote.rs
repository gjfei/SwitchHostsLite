//! Remote hosts: fetch a hosts file over HTTP(S) and refresh it on the
//! interval the item carries, following SwitchHosts' remote entries.

use std::borrow::Cow;
use std::io::Read as _;
use std::time::Duration;

use anyhow::Result;
use rust_i18n::t;

use crate::hosts;
use crate::i18n::Localize as _;

/// Refresh intervals in seconds, as in SwitchHosts' edit dialog; 0 is manual.
pub const INTERVALS: [u64; 7] = [0, 60, 300, 900, 3600, 86400, 604800];
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BYTES: u64 = 4 << 20;

/// Whether `url` is an absolute HTTP(S) URL the fetch channel accepts.
pub fn valid_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")) else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
}

pub fn interval_label(secs: u64) -> Cow<'static, str> {
    match secs {
        0 => t!("refresh_never"),
        60 => t!("refresh_1m"),
        300 => t!("refresh_5m"),
        900 => t!("refresh_15m"),
        3600 => t!("refresh_1h"),
        86400 => t!("refresh_1d"),
        604800 => t!("refresh_1w"),
        _ => t!("refresh_secs", count = secs),
    }
}

/// Download the hosts text, normalized like every other content in the app.
pub fn fetch(url: &str) -> Result<String> {
    let mut body = crate::http::get(url, FETCH_TIMEOUT, MAX_BYTES).localize("err_remote_fetch")?;
    let mut text = String::new();
    body.read_to_string(&mut text).localize("err_remote_fetch")?;
    body.finish().localize("err_remote_fetch")?;
    Ok(hosts::normalize(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_validation() {
        for url in ["https://example.com/hosts", "http://a/b?c", "https://host:8080/x#y"] {
            assert!(valid_url(url), "{url}");
        }
        for url in ["", "example.com/hosts", "ftp://a/b", "https://", "http:///x", "file:///etc/hosts"] {
            assert!(!valid_url(url), "{url}");
        }
    }

    #[test]
    fn intervals_match_upstream() {
        assert_eq!(INTERVALS, [0, 60, 300, 900, 3600, 86400, 604800]);
        assert_eq!(interval_label(0), t!("refresh_never"));
        assert_eq!(interval_label(123), t!("refresh_secs", count = 123));
    }
}