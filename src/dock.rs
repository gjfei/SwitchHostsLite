//! The CDP debugging pipe used to mark a launched browser's Dock tile on
//! macOS. `--remote-debugging-pipe` speaks null-delimited JSON over fds 3/4,
//! so no network port is opened.

/// Standard base64 with padding, for the tile image parameter.
#[cfg(any(target_os = "macos", test))]
pub fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(target_os = "macos")]
mod imp {
    use std::{
        fs::File,
        io::{Read, Write},
        os::fd::{AsRawFd as _, FromRawFd, RawFd},
        process::Command,
        thread,
        time::{Duration, Instant},
    };

    unsafe extern "C" {
        fn pipe(fds: *mut i32) -> i32;
        fn dup2(fd: i32, to: i32) -> i32;
        fn close(fd: i32) -> i32;
        // Variadic: on arm64 the extra argument passes on the stack, so a
        // fixed three-argument prototype would read garbage for F_DUPFD/F_SETFL.
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }

    // macOS fcntl commands and the nonblocking flag.
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const F_DUPFD: i32 = 0;
    const O_NONBLOCK: i32 = 0x0004;

    /// Move a fresh pipe end above the debugging fds, so dup2 onto 3/4 in the
    /// child cannot alias an original that the close loop then kills.
    unsafe fn above_debug_fds(fd: RawFd) -> RawFd {
        if fd >= 5 {
            return fd;
        }
        // SAFETY: plain fcntl/close on an fd this process just created.
        unsafe {
            let moved = fcntl(fd, F_DUPFD, 5);
            close(fd);
            moved
        }
    }

    pub struct Pipe {
        write: File,
        read: File,
        /// The child's ends of both pipes, still open in this process until
        /// the spawn forked them; closing them afterwards keeps EOF sane.
        child_ends: Option<(RawFd, RawFd)>,
    }

    /// Arm `cmd` with the debugging pipe. `None` when the pipes cannot be
    /// created; launching then proceeds unmarked.
    pub fn prepare(cmd: &mut Command) -> Option<Pipe> {
        use std::os::unix::process::CommandExt;
        let mut a = [0i32; 2];
        let mut b = [0i32; 2];
        // SAFETY: plain libc calls on stack arrays.
        unsafe {
            if pipe(a.as_mut_ptr()) != 0 {
                return None;
            }
            if pipe(b.as_mut_ptr()) != 0 {
                close(a[0]);
                close(a[1]);
                return None;
            }
        }
        let (a_read, a_write, b_read, b_write) = unsafe {
            let fds = [
                above_debug_fds(a[0]),
                above_debug_fds(a[1]),
                above_debug_fds(b[0]),
                above_debug_fds(b[1]),
            ];
            if fds.iter().any(|&fd| fd < 0) {
                for fd in fds.iter().filter(|&&fd| fd >= 0) {
                    close(*fd);
                }
                return None;
            }
            (fds[0], fds[1], fds[2], fds[3])
        };
        cmd.arg("--remote-debugging-pipe");
        // SAFETY: runs between fork and exec in the child; only dup2/close on
        // the pipe fds this process just created.
        unsafe {
            cmd.pre_exec(move || {
                if dup2(a_read, 3) < 0 || dup2(b_write, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in [a_read, a_write, b_read, b_write] {
                    close(fd);
                }
                Ok(())
            });
        }
        // SAFETY: the fds are fresh pipe ends owned by this process.
        unsafe {
            Some(Pipe {
                write: File::from_raw_fd(a_write),
                read: File::from_raw_fd(b_read),
                child_ends: Some((a_read, b_write)),
            })
        }
    }

    impl Pipe {
        pub fn close_child_ends(&mut self) {
            if let Some((read, write)) = self.child_ends.take() {
                // SAFETY: fds created by prepare, closed exactly once.
                unsafe {
                    close(read);
                    close(write);
                }
            }
        }

        /// Send `Browser.setDockTile` with the PNG and wait briefly for the
        /// reply. Best effort: a silent browser only means an unmarked Dock.
        /// The pipe must stay open afterwards: Chrome quits on pipe EOF.
        pub fn set_tile(&mut self, png: &[u8]) {
            let msg = format!(
                "{{\"id\":1,\"method\":\"Browser.setDockTile\",\"params\":{{\"image\":\"{}\"}}}}\0",
                super::base64(png)
            );
            if self.write.write_all(msg.as_bytes()).is_err() {
                eprintln!("setDockTile write failed");
                return;
            }
            // SAFETY: plain fcntl on this process's own pipe fd.
            unsafe {
                let fd = self.read.as_raw_fd();
                let flags = fcntl(fd, F_GETFL, 0);
                if flags >= 0 {
                    fcntl(fd, F_SETFL, flags | O_NONBLOCK);
                }
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut reply = Vec::new();
            let mut buf = [0u8; 4096];
            while Instant::now() < deadline {
                match self.read.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        reply.extend_from_slice(&buf[..n]);
                        if reply.contains(&0) {
                            break;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
            let text = String::from_utf8_lossy(&reply);
            if text.contains("\"error\"") {
                eprintln!("setDockTile failed: {text}");
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{Pipe, prepare};

#[cfg(not(target_os = "macos"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub struct Pipe;

#[cfg(not(target_os = "macos"))]
impl Pipe {
    pub fn set_tile(&self, _png: &[u8]) {}
}

#[cfg(not(target_os = "macos"))]
pub fn prepare(_cmd: &mut std::process::Command) -> Option<Pipe> {
    None
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff, 0xbf]), "+/+/");
    }
}