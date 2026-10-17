//! The protocol owns stdout. The app prints freely (Lisp errors, load
//! diagnostics, `println!` in host code), so before anything boots, fd 1 is
//! moved to a private descriptor for the protocol and pointed at stderr.
//! Each tool call then runs with both fds redirected to an anonymous file,
//! so what it printed comes back to the agent as part of the result.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};

/// Take stdout for the protocol: returns the original stdout, and fd 1
/// writes to stderr from now on.
pub(super) fn take_stdout() -> std::io::Result<File> {
    std::io::stdout().flush()?;
    let protocol = unsafe { libc::dup(1) };
    if protocol < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let protocol = unsafe { File::from_raw_fd(protocol) };
    if unsafe { libc::dup2(2, 1) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(protocol)
}

/// Points fds 1 and 2 back at stderr when dropped, panic or not.
struct Redirect {
    saved_stderr: i32,
}

impl Drop for Redirect {
    fn drop(&mut self) {
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        unsafe {
            libc::dup2(self.saved_stderr, 2);
            libc::dup2(self.saved_stderr, 1);
            libc::close(self.saved_stderr);
        }
    }
}

/// Run `f` with stdout and stderr captured; returns its result and the text
/// it printed, which is also echoed to the real stderr for the server log.
/// A panic in `f` is resumed after the text (the panic message included) is
/// echoed.
pub(super) fn capture_output<R>(f: impl FnOnce() -> R) -> (R, String) {
    let Ok(mut file) = tempfile::tempfile() else { return (f(), String::new()) };
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    let saved_stderr = unsafe { libc::dup(2) };
    if saved_stderr < 0 {
        return (f(), String::new());
    }
    let outcome = {
        let _redirect = Redirect { saved_stderr };
        unsafe {
            libc::dup2(file.as_raw_fd(), 2);
            libc::dup2(file.as_raw_fd(), 1);
        }
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
    };
    let mut text = String::new();
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.read_to_string(&mut text);
    eprint!("{text}");
    match outcome {
        Ok(result) => (result, text),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
