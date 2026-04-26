//! Local listener handles.

#[cfg(windows)]
use std::ffi::OsString;
use std::io;

use crate::{LocalEndpoint, LocalStream, PeerIdentity};

#[cfg(windows)]
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

/// Local IPC listener.
#[cfg(unix)]
#[derive(Debug)]
pub struct LocalListener {
    inner: tokio::net::UnixListener,
}

/// Local IPC listener backed by a Windows named pipe.
#[cfg(windows)]
#[derive(Debug)]
pub struct LocalListener {
    pipe_name: OsString,
    pending: tokio::sync::Mutex<Option<NamedPipeServer>>,
}

impl LocalListener {
    /// Binds a local listener.
    pub fn bind(endpoint: &LocalEndpoint) -> io::Result<Self> {
        bind_impl(endpoint)
    }

    /// Accepts one local client and returns its byte stream plus peer identity.
    pub async fn accept(&self) -> io::Result<(LocalStream, PeerIdentity)> {
        accept_impl(self).await
    }
}

#[cfg(unix)]
fn bind_impl(endpoint: &LocalEndpoint) -> io::Result<LocalListener> {
    Ok(LocalListener {
        inner: tokio::net::UnixListener::bind(endpoint.as_path())?,
    })
}

#[cfg(windows)]
fn bind_impl(endpoint: &LocalEndpoint) -> io::Result<LocalListener> {
    let pipe_name = endpoint.as_pipe_name().to_owned();
    let pending = create_server(&pipe_name, true)?;
    Ok(LocalListener {
        pipe_name,
        pending: tokio::sync::Mutex::new(Some(pending)),
    })
}

#[cfg(unix)]
async fn accept_impl(listener: &LocalListener) -> io::Result<(LocalStream, PeerIdentity)> {
    let (stream, _addr) = listener.inner.accept().await?;
    let peer = PeerIdentity::from_unix_stream(&stream)?;
    Ok((stream, peer))
}

#[cfg(windows)]
async fn accept_impl(listener: &LocalListener) -> io::Result<(LocalStream, PeerIdentity)> {
    let server = {
        let mut pending = listener.pending.lock().await;
        pending
            .take()
            .ok_or_else(|| io::Error::other("named-pipe accept already in progress"))?
    };

    server.connect().await?;
    let next = create_server(&listener.pipe_name, false)?;
    {
        let mut pending = listener.pending.lock().await;
        *pending = Some(next);
    }

    Ok((server, PeerIdentity::current_process()))
}

#[cfg(windows)]
fn create_server(pipe_name: &OsString, first_instance: bool) -> io::Result<NamedPipeServer> {
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first_instance);
    options.create(pipe_name)
}
