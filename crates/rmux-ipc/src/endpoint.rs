//! Local endpoint naming.

use std::ffi::OsStr;
#[cfg(unix)]
use std::ffi::OsString;
use std::io;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

#[cfg(windows)]
use rmux_os::identity::{IdentityResolver, UserIdentity};

const DEFAULT_SOCKET_LABEL: &str = "default";
#[cfg(unix)]
const FALLBACK_SOCKET_ROOT: &str = "/tmp";
const RMUX_ENV: &str = "RMUX";
#[cfg(unix)]
const RMUX_TMPDIR_ENV: &str = "RMUX_TMPDIR";
const SOCKET_DIR_PREFIX: &str = "rmux";
#[cfg(windows)]
const PIPE_PREFIX: &str = r"\\.\pipe\";

/// Address of a local RMUX endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocalEndpoint {
    path: PathBuf,
}

impl LocalEndpoint {
    /// Builds an endpoint from an explicit Unix socket path.
    #[must_use]
    pub fn from_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the Unix socket path for this endpoint.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// Consumes the endpoint into its Unix socket path.
    #[must_use]
    pub fn into_path(self) -> PathBuf {
        self.path
    }

    /// Returns the Windows named-pipe path for this endpoint.
    #[cfg(windows)]
    #[must_use]
    pub fn as_pipe_name(&self) -> &OsStr {
        self.path.as_os_str()
    }
}

/// Computes the default RMUX endpoint.
pub fn default_endpoint() -> io::Result<LocalEndpoint> {
    endpoint_for_label(DEFAULT_SOCKET_LABEL)
}

/// Computes an RMUX endpoint for a top-level `-L` socket name.
pub fn endpoint_for_label(label: impl AsRef<OsStr>) -> io::Result<LocalEndpoint> {
    endpoint_for_label_impl(label.as_ref())
}

#[cfg(unix)]
fn endpoint_for_label_impl(label: &OsStr) -> io::Result<LocalEndpoint> {
    let user_id = rmux_os::identity::real_user_id();
    endpoint_from_parts(std::env::var_os(RMUX_TMPDIR_ENV).as_deref(), user_id, label)
}

#[cfg(windows)]
fn endpoint_for_label_impl(label: &OsStr) -> io::Result<LocalEndpoint> {
    let identity = IdentityResolver::current()?;
    let UserIdentity::Sid(sid) = identity else {
        return Err(io::Error::other(
            "Windows identity resolver returned a non-SID identity",
        ));
    };
    let label = pipe_component(label);
    let sid = pipe_component(OsStr::new(sid.as_ref()));
    Ok(LocalEndpoint::from_path(PathBuf::from(format!(
        "{PIPE_PREFIX}{SOCKET_DIR_PREFIX}-{sid}-{label}"
    ))))
}

#[cfg(unix)]
fn endpoint_from_parts(
    rmux_tmpdir: Option<&OsStr>,
    user_id: u32,
    label: &OsStr,
) -> io::Result<LocalEndpoint> {
    let root = socket_root_from_parts(rmux_tmpdir)?;
    let base = root.join(format!("{SOCKET_DIR_PREFIX}-{user_id}"));
    let mut path = os_string_into_bytes(base.into_os_string());
    path.push(b'/');
    path.extend_from_slice(os_str_bytes(label).as_ref());

    Ok(LocalEndpoint::from_path(path_buf_from_bytes(path)))
}

/// Resolves the top-level endpoint from `-L`, `-S`, `$RMUX`, or defaults.
///
/// `-S` wins over `-L`; both command-line forms win over `$RMUX`.
pub fn resolve_endpoint(
    socket_name: Option<&OsStr>,
    socket_path: Option<&Path>,
) -> io::Result<LocalEndpoint> {
    if let Some(socket_path) = socket_path {
        return Ok(LocalEndpoint::from_path(socket_path.to_path_buf()));
    }
    if let Some(socket_name) = socket_name {
        return endpoint_for_label(socket_name);
    }
    if let Some(socket_path) = socket_path_from_rmux_env(std::env::var_os(RMUX_ENV).as_deref()) {
        return Ok(LocalEndpoint::from_path(socket_path));
    }

    default_endpoint()
}

/// Resolves the root directory used for RMUX sockets.
#[cfg(unix)]
pub fn socket_root_from_parts(rmux_tmpdir: Option<&OsStr>) -> io::Result<PathBuf> {
    let rmux_tmpdir = rmux_tmpdir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let candidates = rmux_tmpdir
        .into_iter()
        .chain(std::iter::once(PathBuf::from(FALLBACK_SOCKET_ROOT)));

    for candidate in candidates {
        if let Ok(resolved) = std::fs::canonicalize(&candidate) {
            return Ok(resolved);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no suitable rmux socket directory",
    ))
}

fn socket_path_from_rmux_env(rmux: Option<&OsStr>) -> Option<PathBuf> {
    let rmux = rmux?;
    let bytes = os_str_bytes(rmux);
    if bytes.is_empty() || bytes.first() == Some(&b',') {
        return None;
    }

    let end = bytes
        .iter()
        .position(|byte| *byte == b',')
        .unwrap_or(bytes.len());
    let path = path_buf_from_bytes(bytes[..end].to_vec());
    socket_path_is_rmux_owned(&path).then_some(path)
}

fn socket_path_is_rmux_owned(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            name.starts_with(SOCKET_DIR_PREFIX) && name[SOCKET_DIR_PREFIX.len()..].starts_with('-')
        })
}

#[cfg(unix)]
fn os_str_bytes(value: &OsStr) -> Vec<u8> {
    value.as_bytes().to_vec()
}

#[cfg(unix)]
fn os_string_into_bytes(value: OsString) -> Vec<u8> {
    value.into_vec()
}

#[cfg(unix)]
fn path_buf_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(bytes))
}

#[cfg(windows)]
fn os_str_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_buf_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(windows)]
fn pipe_component(value: &OsStr) -> String {
    let mut component = String::new();
    for ch in value.to_string_lossy().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            component.push(ch);
        } else {
            component.push('_');
        }
    }
    if component.is_empty() {
        DEFAULT_SOCKET_LABEL.to_owned()
    } else {
        component
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::ffi::OsStr;

    #[cfg(unix)]
    #[test]
    fn default_endpoint_uses_the_spec_layout() {
        let path = default_endpoint().expect("default endpoint").into_path();
        let path_string = path.to_string_lossy();

        assert!(path_string.ends_with("/default"));
        assert!(path_string.contains("/rmux-"));
    }

    #[cfg(unix)]
    #[test]
    fn unresolved_rmux_tmpdir_falls_back_to_tmp() {
        assert_eq!(
            socket_root_from_parts(Some(OsStr::new(
                "relative-rmux-test-path-that-does-not-exist"
            )))
            .expect("socket root"),
            std::fs::canonicalize("/tmp").expect("canonical /tmp")
        );
    }

    #[cfg(windows)]
    #[test]
    fn default_endpoint_uses_a_user_scoped_named_pipe() {
        let path = default_endpoint()
            .expect("default named-pipe endpoint")
            .into_path();
        let path = path.to_string_lossy();

        assert!(path.starts_with(r"\\.\pipe\rmux-S-"));
        assert!(path.ends_with("-default"));
    }

    #[cfg(windows)]
    #[test]
    fn pipe_labels_are_sanitized() {
        assert_eq!(
            pipe_component(OsStr::new("alpha/beta:gamma")),
            "alpha_beta_gamma"
        );
    }
}
