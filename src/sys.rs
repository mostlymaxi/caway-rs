//! The three libc calls std does not expose. Declared by hand so the crate
//! needs no dependencies; this program is Linux-only anyway.

use std::ffi::{c_int, c_ulong};
use std::io;

#[repr(C)]
pub struct PollFd {
    pub fd: c_int,
    pub events: i16,
    pub revents: i16,
}

pub const POLLIN: i16 = 0x001;

mod ffi {
    use super::*;
    extern "C" {
        pub fn poll(fds: *mut PollFd, nfds: c_ulong, timeout: c_int) -> c_int;
        pub fn prctl(option: c_int, ...) -> c_int;
        pub fn getuid() -> u32;
    }
}

/// Blocks until one of `fds` is ready or `timeout_ms` elapses (-1 = forever).
pub fn poll(fds: &mut [PollFd], timeout_ms: i32) -> io::Result<usize> {
    loop {
        // SAFETY: `fds` is a valid, exclusively borrowed slice of repr(C) pollfd
        // structs and its length is passed along.
        let r = unsafe { ffi::poll(fds.as_mut_ptr(), fds.len() as c_ulong, timeout_ms) };
        if r >= 0 {
            return Ok(r as usize);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

pub fn getuid() -> u32 {
    // SAFETY: getuid cannot fail and touches no memory.
    unsafe { ffi::getuid() }
}

/// Asks the kernel to SIGTERM the calling process when its parent dies.
/// Meant for `pre_exec`, so a child never outlives us. Async-signal-safe.
pub fn die_with_parent() {
    const PR_SET_PDEATHSIG: c_int = 1;
    const SIGTERM: c_ulong = 15;
    // SAFETY: plain syscall wrapper with constant arguments.
    unsafe {
        ffi::prctl(PR_SET_PDEATHSIG, SIGTERM);
    }
}
