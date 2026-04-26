//! Local stream handles.

#[cfg(windows)]
use std::ffi::OsString;
use std::io;
#[cfg(windows)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use crate::LocalEndpoint;

#[cfg(windows)]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
#[cfg(windows)]
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

/// Identity of a connected local peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerIdentity {
    /// Peer process id.
    pub pid: u32,
    /// Peer Unix user id.
    pub uid: u32,
}

/// Async local byte stream used by the server runtime.
#[cfg(unix)]
pub type LocalStream = tokio::net::UnixStream;

/// Blocking local byte stream used by the CLI.
#[cfg(unix)]
pub type BlockingLocalStream = std::os::unix::net::UnixStream;

/// Async local byte stream default_value for Windows until named pipes are added.
#[cfg(windows)]
pub type LocalStream = tokio::net::windows::named_pipe::NamedPipeServer;

/// Blocking local byte stream used by the CLI.
#[cfg(windows)]
pub struct BlockingLocalStream {
    inner: NamedPipeClient,
    runtime: tokio::runtime::Runtime,
}

#[cfg(unix)]
impl PeerIdentity {
    pub(crate) fn from_unix_stream(stream: &LocalStream) -> io::Result<Self> {
        let credentials = stream.peer_cred()?;
        let pid = credentials
            .pid()
            .ok_or_else(|| io::Error::other("unix peer credentials did not include a pid"))?;
        let uid = credentials.uid();
        let pid = u32::try_from(pid)
            .map_err(|_| io::Error::other(format!("invalid unix peer pid {pid}")))?;
        Ok(Self { pid, uid })
    }
}

#[cfg(windows)]
impl PeerIdentity {
    pub(crate) fn current_process() -> Self {
        Self {
            pid: std::process::id(),
            uid: 0,
        }
    }
}

#[cfg(unix)]
/// Connects a blocking client stream to a local endpoint.
pub fn connect_blocking(
    endpoint: &LocalEndpoint,
    timeout: Duration,
) -> io::Result<BlockingLocalStream> {
    #[cfg(not(target_os = "linux"))]
    use std::os::fd::AsRawFd;

    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    use rustix::net::sockopt::socket_error;
    use rustix::net::{
        connect as socket_connect, socket_with, AddressFamily, SocketAddrUnix, SocketType,
    };

    let socket_path = endpoint.as_path();
    let address = SocketAddrUnix::new(socket_path)?;
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        socket_creation_flags(),
        None,
    )?;
    configure_socket_for_connect(&socket)?;

    match socket_connect(&socket, &address) {
        Ok(()) => {}
        Err(rustix::io::Errno::INPROGRESS | rustix::io::Errno::WOULDBLOCK) => {
            wait_for_connect_completion(socket_path, timeout, |remaining| {
                let poll_timeout = Timespec {
                    tv_sec: remaining.as_secs() as i64,
                    tv_nsec: remaining.subsec_nanos().into(),
                };
                let mut fds = [PollFd::new(
                    &socket,
                    PollFlags::OUT | PollFlags::ERR | PollFlags::HUP,
                )];

                match poll(&mut fds, Some(&poll_timeout)) {
                    Ok(0) => Ok(ConnectProgress::Pending),
                    Ok(_) => Ok(ConnectProgress::Ready),
                    Err(rustix::io::Errno::INTR) => Ok(ConnectProgress::Pending),
                    Err(error) => Err(error.into()),
                }
            })?;
        }
        Err(error) => return Err(error.into()),
    }

    match socket_error(&socket)? {
        Ok(()) => {}
        Err(error) => return Err(error.into()),
    }

    let stream = BlockingLocalStream::from(socket);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(windows)]
/// Connects a blocking client stream to a local endpoint.
pub fn connect_blocking(
    endpoint: &LocalEndpoint,
    timeout: Duration,
) -> io::Result<BlockingLocalStream> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let deadline = Instant::now() + timeout;
    let pipe_name = endpoint.as_pipe_name().to_owned();
    loop {
        match runtime.block_on(open_named_pipe_client(&pipe_name)) {
            Ok(inner) => return Ok(BlockingLocalStream { inner, runtime }),
            Err(error) if connect_retryable(&error) => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "timed out after {}s connecting to '{}'",
                            timeout.as_secs_f32(),
                            endpoint.as_path().display()
                        ),
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn connect_retryable(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
}

#[cfg(windows)]
async fn open_named_pipe_client(pipe_name: &OsString) -> io::Result<NamedPipeClient> {
    ClientOptions::new().open(pipe_name)
}

#[cfg(windows)]
impl Read for BlockingLocalStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.runtime.block_on(self.inner.read(buf))
    }
}

#[cfg(windows)]
impl Write for BlockingLocalStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.runtime.block_on(self.inner.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.runtime.block_on(self.inner.flush())
    }
}

#[cfg(target_os = "linux")]
fn socket_creation_flags() -> rustix::net::SocketFlags {
    rustix::net::SocketFlags::CLOEXEC | rustix::net::SocketFlags::NONBLOCK
}

#[cfg(all(unix, not(target_os = "linux")))]
fn socket_creation_flags() -> rustix::net::SocketFlags {
    rustix::net::SocketFlags::empty()
}

#[cfg(target_os = "linux")]
fn configure_socket_for_connect<Fd>(_socket: &Fd) -> io::Result<()>
where
    Fd: std::os::fd::AsFd,
{
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn configure_socket_for_connect<Fd>(socket: &Fd) -> io::Result<()>
where
    Fd: std::os::fd::AsFd,
{
    let raw_fd = socket.as_fd().as_raw_fd();
    set_fd_flag(raw_fd, libc::FD_CLOEXEC)?;
    set_status_flag(raw_fd, libc::O_NONBLOCK)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn set_fd_flag(raw_fd: libc::c_int, flag: libc::c_int) -> io::Result<()> {
    let flags = unsafe {
        // SAFETY: `fcntl` reads descriptor flags from a valid socket fd.
        libc::fcntl(raw_fd, libc::F_GETFD)
    };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }

    let result = unsafe {
        // SAFETY: `fcntl` updates only descriptor flags for the same valid fd.
        libc::fcntl(raw_fd, libc::F_SETFD, flags | flag)
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn set_status_flag(raw_fd: libc::c_int, flag: libc::c_int) -> io::Result<()> {
    let flags = unsafe {
        // SAFETY: `fcntl` reads file status flags from a valid socket fd.
        libc::fcntl(raw_fd, libc::F_GETFL)
    };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }

    let result = unsafe {
        // SAFETY: `fcntl` updates only file status flags for the same valid fd.
        libc::fcntl(raw_fd, libc::F_SETFL, flags | flag)
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectProgress {
    Pending,
    Ready,
}

#[cfg(unix)]
fn wait_for_connect_completion<P>(
    socket_path: &Path,
    timeout: Duration,
    mut wait_for_ready: P,
) -> io::Result<()>
where
    P: FnMut(Duration) -> io::Result<ConnectProgress>,
{
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "timed out after {}s connecting to '{}'",
                    timeout.as_secs_f32(),
                    socket_path.display()
                ),
            ));
        }

        if wait_for_ready(remaining)? == ConnectProgress::Ready {
            return Ok(());
        }
    }
}
