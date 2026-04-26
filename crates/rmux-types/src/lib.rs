#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Portable semantic newtypes shared by non-adjacent RMUX crates.

/// A terminal geometry request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TerminalSize {
    /// The requested column count.
    pub cols: u16,
    /// The requested row count.
    pub rows: u16,
}

impl TerminalSize {
    /// Creates a terminal size value from column and row counts.
    #[must_use]
    pub const fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }
}
