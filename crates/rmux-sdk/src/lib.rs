#![deny(missing_docs)]
#![forbid(unsafe_code)]

//! Public daemon-backed RMUX SDK scaffolding.
//!
//! v1 introduces a fully daemon-backed public SDK. This crate currently
//! exposes only the compile-time vocabulary and facade-error skeletons
//! needed by later steps; daemon transport, handle types, and event
//! plumbing land in subsequent commits.
//!
//! `rmux-sdk` is a public integration peer of `rmux-client` and must not
//! depend on `rmux-client`, `rmux-core`, `rmux-server`, or `rmux-pty` as
//! normal dependencies. Final typed identifiers (`SessionId`, `WindowId`,
//! `PaneId`) are owned by `rmux-proto` (Milestone 6) and will be re-exported
//! from this crate at that point.

pub mod error;
pub mod types;

pub use error::{Result, RmuxError};
pub use types::{RmuxEndpoint, SessionName};
