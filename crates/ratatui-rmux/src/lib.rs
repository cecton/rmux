#![deny(missing_docs)]
#![forbid(unsafe_code)]

//! Public ratatui integration crate for RMUX v1.
//!
//! `ratatui-rmux` provides three intentionally narrow building blocks
//! around `rmux-sdk`:
//!
//! * [`PaneDriver`] is the async owner of pane event I/O and state
//!   mutation. It is the *only* place RMUX behaviour is reached in
//!   this crate; it goes through `rmux-sdk` and never touches
//!   `rmux-client`, `rmux-core`, `rmux-server`, or `rmux-pty`.
//! * [`PaneState`] is the deterministic, sync, plain-data projection
//!   the driver folds events into. The same value renders the same
//!   buffer cells every time.
//! * [`PaneWidget`] is the sync ratatui widget that paints a
//!   `PaneState` into a ratatui [`Buffer`]. It performs no I/O and
//!   has no time/clock dependencies.
//!
//! The async/sync split keeps the widget safe to call from any
//! ratatui draw loop — including non-tokio hosts and unit tests —
//! while still letting the daemon-backed driver advance state
//! between draws.
//!
//! The recorded production source/dependency budget for this crate
//! lives in `spec/runtime.yaml` and is enforced by
//! `crates/ratatui-rmux/tests/budget.rs` plus
//! `scripts/ratatui-rmux-budget.sh`.
//!
//! [`Buffer`]: ratatui_core::buffer::Buffer

pub mod driver;
pub mod state;
pub mod theme;
pub mod widget;

pub use driver::PaneDriver;
pub use state::{PaneLifecycle, PaneState};
pub use theme::{cell_style, color, glyph_symbol, modifier};
pub use widget::PaneWidget;
