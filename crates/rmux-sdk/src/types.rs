//! SDK type vocabulary skeleton.
//!
//! This module fixes the shapes that downstream SDK steps build on without
//! taking ownership of identity types. Authoritative typed identifiers
//! (`SessionId`, `WindowId`, `PaneId`) land in `rmux-proto` in Milestone 6 and
//! are re-exported here once they exist; until then this skeleton only
//! re-exports the already-stable [`SessionName`] from `rmux-proto` and
//! defines the public endpoint vocabulary used by builder/bootstrap code.

use std::path::PathBuf;

pub use rmux_proto::SessionName;

/// Selects the daemon endpoint resolution strategy used by the SDK.
///
/// `Default` defers to platform defaults resolved through the existing
/// RMUX OS layer. The explicit variants carry caller-supplied paths/names
/// and bypass the auto-discovery allowlist while still preserving the
/// daemon's own permission and symlink checks.
///
/// Marked `#[non_exhaustive]` because additional transports (such as TCP
/// or test-harness in-memory pipes) are anticipated in later steps and
/// must be addable without breaking downstream pattern matches.
#[derive(Debug, Default, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RmuxEndpoint {
    /// Resolve the platform default endpoint via the OS/IPC layer.
    #[default]
    Default,
    /// Use an explicit Unix domain socket path.
    UnixSocket(PathBuf),
    /// Use an explicit Windows named pipe identifier.
    WindowsPipe(String),
}

impl RmuxEndpoint {
    /// Returns `true` when this endpoint defers to platform default
    /// resolution.
    #[must_use]
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }
}
