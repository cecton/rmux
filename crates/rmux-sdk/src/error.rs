//! SDK facade error skeleton.
//!
//! `RmuxError` is the SDK-facing error: it is intentionally not `Clone`,
//! exposes visible recovery hints, and is the boundary at which lower-crate
//! typed unsupported operations are mapped. Milestone 11 expands the variant
//! set and adds `CollectError` plus the full `Result<T>` ecosystem; this
//! skeleton fixes only the shape needed by the compile-time contract gate.

use std::error::Error;
use std::fmt;

/// SDK facade error type for daemon-backed operations.
///
/// The variant set is intentionally minimal during the v1 scaffold; new
/// variants are added in later steps. Variants must remain constructible
/// without `Clone`, so additions should hold owned diagnostics rather than
/// introducing cloneable inner errors as the only construction path. The type is
/// deliberately not `Clone`: error surfaces that need duplication should
/// wrap in `Arc` rather than fan out cheap copies of opaque diagnostics.
#[derive(Debug)]
#[non_exhaustive]
pub enum RmuxError {
    /// A capability or operation is not supported by the negotiated
    /// daemon. Carries a stable feature identifier and a visible recovery
    /// hint so the SDK can map lower-crate typed unsupported errors to a
    /// consistent surface.
    Unsupported {
        /// Stable, machine-readable identifier for the unsupported
        /// operation. Used by callers that pattern-match on capabilities.
        feature: String,
        /// Visible recovery hint shown after the human-readable message.
        hint: String,
    },
}

impl RmuxError {
    /// Creates an unsupported-feature error with a stable identifier and
    /// visible recovery hint.
    #[must_use]
    pub fn unsupported(feature: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::Unsupported {
            feature: feature.into(),
            hint: hint.into(),
        }
    }

    /// Returns the visible recovery hint associated with this error,
    /// if one is recorded for the variant.
    ///
    /// Future variants that have no recovery suggestion should return
    /// `None` so callers can branch on the presence of guidance.
    #[must_use]
    pub fn hint(&self) -> Option<&str> {
        match self {
            Self::Unsupported { hint, .. } => Some(hint),
        }
    }

    /// Returns the stable feature identifier when the error variant carries
    /// one. The identifier is intended for log keys and capability matching,
    /// not user-facing copy.
    #[must_use]
    pub fn feature(&self) -> Option<&str> {
        match self {
            Self::Unsupported { feature, .. } => Some(feature),
        }
    }
}

impl fmt::Display for RmuxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { feature, hint } => {
                write!(formatter, "unsupported feature `{feature}`\nhint: {hint}")
            }
        }
    }
}

impl Error for RmuxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        // The skeleton variants are leaf errors; later wrapping variants
        // override this to expose their underlying cause.
        match self {
            Self::Unsupported { .. } => None,
        }
    }
}

/// SDK result alias parameterised over the SDK facade [`RmuxError`].
///
/// Milestone 11 finalises the alias alongside `CollectError`; this skeleton
/// only stabilises the type name so later modules can use it without an
/// incompatible rename.
pub type Result<T> = core::result::Result<T, RmuxError>;
