use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::process::{
    getpid, ioctl_tiocsctty, kill_process as rustix_kill_process, kill_process_group, setsid,
};
use rustix::termios::{tcgetwinsize, tcsetpgrp, tcsetwinsize};

use crate::{size, ProcessId, Result, Signal, TerminalSize};

pub(crate) fn open_pty_pair() -> Result<(OwnedFd, OwnedFd)> {
    let mut master = MaybeUninit::<libc::c_int>::uninit();
    let mut slave = MaybeUninit::<libc::c_int>::uninit();

    // SAFETY: `openpty` initializes both fd out-pointers on success. The name,
    // termios, and winsize pointers are null because RMUX configures size via
    // the shared resize path after allocation.
    let result = unsafe {
        libc::openpty(
            master.as_mut_ptr(),
            slave.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if result == -1 {
        return Err(last_errno().into());
    }

    // SAFETY: `openpty` returned success, so both descriptors are initialized
    // and owned by this process.
    let master = unsafe { OwnedFd::from_raw_fd(master.assume_init()) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave.assume_init()) };

    set_cloexec(master.as_raw_fd())?;
    set_cloexec(slave.as_raw_fd())?;

    Ok((master, slave))
}

pub(crate) fn query_size(fd: BorrowedFd<'_>) -> Result<TerminalSize> {
    Ok(size::from_winsize(tcgetwinsize(fd)?))
}

pub(crate) fn apply_size(fd: BorrowedFd<'_>, size: TerminalSize) -> Result<()> {
    tcsetwinsize(fd, size::into_winsize(size))?;
    Ok(())
}

pub(crate) fn setup_child_controlling_terminal(raw_master_fd: RawFd) -> std::io::Result<()> {
    // SAFETY: This closes only the child process' inherited copy of the PTY
    // master fd. The parent still owns its separate descriptor.
    unsafe { rustix::io::close(raw_master_fd) };

    setsid().map_err(std::io::Error::from)?;

    // SAFETY: `stdin` has already been wired to the PTY slave by `Command`, so
    // fd 0 is a valid borrowed descriptor for the rest of the pre-exec setup.
    let slave_stdin = unsafe { BorrowedFd::borrow_raw(0) };
    ioctl_tiocsctty(slave_stdin).map_err(std::io::Error::from)?;
    tcsetpgrp(slave_stdin, getpid()).map_err(std::io::Error::from)?;

    Ok(())
}

pub(crate) fn kill_foreground_process_group(pid: ProcessId, signal: Signal) -> Result<()> {
    kill_process_group(pid.as_rustix_pid()?, signal.as_rustix_signal())?;
    Ok(())
}

pub(crate) fn kill_process(pid: ProcessId, signal: Signal) -> Result<()> {
    rustix_kill_process(pid.as_rustix_pid()?, signal.as_rustix_signal())?;
    Ok(())
}

pub(crate) fn read(fd: BorrowedFd<'_>, buffer: &mut [u8]) -> io::Result<usize> {
    rustix::io::read(fd, buffer).map_err(io::Error::from)
}

pub(crate) fn write_all(fd: BorrowedFd<'_>, mut buffer: &[u8]) -> io::Result<()> {
    while !buffer.is_empty() {
        match rustix::io::write(fd, buffer) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0")),
            Ok(bytes_written) => buffer = &buffer[bytes_written..],
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
    }

    Ok(())
}

pub(crate) fn set_nonblocking(fd: BorrowedFd<'_>) -> io::Result<()> {
    let flags = fcntl_getfl(fd).map_err(io::Error::other)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(io::Error::other)?;
    Ok(())
}

fn set_cloexec(fd: RawFd) -> Result<()> {
    // SAFETY: `fcntl` is called with a valid fd owned by the caller.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(last_errno().into());
    }

    // SAFETY: `fcntl` mutates only fd flags for a valid descriptor.
    let result = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if result == -1 {
        return Err(last_errno().into());
    }

    Ok(())
}

fn last_errno() -> rustix::io::Errno {
    let raw = std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO);
    rustix::io::Errno::from_raw_os_error(raw)
}
