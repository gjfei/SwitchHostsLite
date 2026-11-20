//! HTTP(S) GET through the curl that ships with macOS and Windows 10+, which
//! keeps a TLS stack out of the binary. Shared by the updater and remote hosts.

use std::io::{self, Read};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

const USER_AGENT: &str = concat!("SwitchHostsLite/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// A response body streamed from curl's stdout.
pub struct Body {
    child: Child,
    stdout: ChildStdout,
    received: u64,
    max_len: u64,
}

#[cfg(target_os = "macos")]
fn curl() -> Command {
    Command::new("/usr/bin/curl")
}

#[cfg(target_os = "windows")]
fn curl() -> Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system = std::env::var_os("SystemRoot")
        .map(|root| std::path::PathBuf::from(root).join("System32").join("curl.exe"))
        .filter(|curl| curl.is_file());
    let mut command = Command::new(system.as_deref().unwrap_or("curl.exe".as_ref()));
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn curl() -> Command {
    Command::new("curl")
}

/// Start a GET of `url` that fails on an HTTP error status, after
/// `timeout` or when the body is longer than `max_len`.
pub fn get(url: &str, timeout: Duration, max_len: u64) -> io::Result<Body> {
    let mut child = curl()
        // Must come first: ignores the user's .curlrc.
        .arg("--disable")
        .args(["--silent", "--show-error", "--fail", "--location", "--max-redirs", "10"])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .arg("--connect-timeout")
        .arg(CONNECT_TIMEOUT.as_secs().to_string())
        .arg("--max-time")
        .arg(timeout.as_secs().to_string())
        .arg("--max-filesize")
        .arg(max_len.to_string())
        .arg("--user-agent")
        .arg(USER_AGENT)
        .arg("--url")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| io::Error::new(err.kind(), format!("curl: {err}")))?;
    let stdout = child.stdout.take().expect("piped stdout");
    Ok(Body { child, stdout, received: 0, max_len })
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.stdout.read(buf)?;
        self.received += n as u64;
        if self.received > self.max_len {
            return Err(io::Error::other(format!("response longer than {} bytes", self.max_len)));
        }
        Ok(n)
    }
}

impl Body {
    /// Once the body was read to the end: curl's error if the transfer failed.
    pub fn finish(mut self) -> io::Result<()> {
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let status = self.child.wait()?;
        if status.success() {
            return Ok(());
        }
        Err(io::Error::other(format!("curl: {status} {}", stderr.trim())))
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}