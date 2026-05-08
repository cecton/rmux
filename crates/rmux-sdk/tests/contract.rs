//! Compile-only contract assertions for SDK vocabulary and the facade error.
//!
//! These checks intentionally avoid daemon startup, IPC, filesystem
//! endpoints, and runtime I/O; they exist purely to gate the static bounds
//! and derive surface required by later async/transport layers. The bounds
//! are enforced at compile time via [`_assert_bounds`] (a never-called
//! function that the linker may discard, but which the type-checker still
//! validates when this integration test target is compiled), and additionally
//! rehearsed inside `#[test]` bodies so a single regression surfaces with a
//! clear failure name.

#![allow(dead_code, clippy::extra_unused_type_parameters)]

use std::collections::hash_map::DefaultHasher;
use std::error::Error as StdError;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use rmux_sdk::{Result, RmuxEndpoint, RmuxError, SessionName};

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
fn assert_static<T: 'static>() {}
fn assert_error<T: StdError + 'static>() {}
fn assert_default<T: Default>() {}
fn assert_clone<T: Clone>() {}
fn assert_eq_hash<T: Eq + Hash>() {}
fn assert_debug<T: std::fmt::Debug>() {}

/// Compile-only gate for the SDK's static bounds and expected derives.
///
/// This function is never invoked; its purpose is to fail compilation if
/// any of the listed type bounds regress while compiling this integration
/// test target. The runtime tests below repeat the important bounds with
/// targeted failure names.
fn _assert_bounds() {
    assert_send::<RmuxEndpoint>();
    assert_sync::<RmuxEndpoint>();
    assert_static::<RmuxEndpoint>();
    assert_default::<RmuxEndpoint>();
    assert_clone::<RmuxEndpoint>();
    assert_eq_hash::<RmuxEndpoint>();
    assert_debug::<RmuxEndpoint>();

    assert_send::<SessionName>();
    assert_sync::<SessionName>();
    assert_static::<SessionName>();

    assert_send::<RmuxError>();
    assert_sync::<RmuxError>();
    assert_static::<RmuxError>();
    assert_error::<RmuxError>();
    assert_debug::<RmuxError>();

    assert_send::<Result<RmuxEndpoint>>();
    assert_sync::<Result<RmuxEndpoint>>();
    assert_static::<Result<RmuxEndpoint>>();
}

#[test]
fn endpoint_vocabulary_is_send_sync_static() {
    assert_send::<RmuxEndpoint>();
    assert_sync::<RmuxEndpoint>();
    assert_static::<RmuxEndpoint>();
}

#[test]
fn session_name_re_export_is_send_sync_static() {
    assert_send::<SessionName>();
    assert_sync::<SessionName>();
    assert_static::<SessionName>();
}

#[test]
fn facade_error_is_send_sync_static_error() {
    assert_send::<RmuxError>();
    assert_sync::<RmuxError>();
    assert_static::<RmuxError>();
    assert_error::<RmuxError>();
}

#[test]
fn endpoint_default_is_default_variant() {
    let endpoint = RmuxEndpoint::default();
    assert_eq!(endpoint, RmuxEndpoint::Default);
    assert!(endpoint.is_default());
    assert!(!RmuxEndpoint::WindowsPipe("rmux".to_owned()).is_default());
}

#[test]
fn endpoint_explicit_variants_round_trip_equality_and_hash() {
    let unix_a = RmuxEndpoint::UnixSocket(PathBuf::from("/tmp/rmux.sock"));
    let unix_b = RmuxEndpoint::UnixSocket(PathBuf::from("/tmp/rmux.sock"));
    let unix_c = RmuxEndpoint::UnixSocket(PathBuf::from("/tmp/other.sock"));
    let pipe = RmuxEndpoint::WindowsPipe("rmux".to_owned());

    assert_eq!(unix_a, unix_b);
    assert_ne!(unix_a, unix_c);
    assert_ne!(unix_a, pipe);
    assert_ne!(unix_a, RmuxEndpoint::Default);
    assert_eq!(unix_a, unix_a.clone());

    let mut hasher_a = DefaultHasher::new();
    unix_a.hash(&mut hasher_a);
    let mut hasher_b = DefaultHasher::new();
    unix_b.hash(&mut hasher_b);
    assert_eq!(
        hasher_a.finish(),
        hasher_b.finish(),
        "Eq-equal endpoints must produce equal hashes"
    );
}

#[test]
fn result_alias_threads_facade_error() {
    fn always_default() -> Result<RmuxEndpoint> {
        Ok(RmuxEndpoint::default())
    }
    assert!(matches!(always_default(), Ok(RmuxEndpoint::Default)));
}

#[test]
fn facade_error_display_pins_feature_and_hint_layout() {
    let error = RmuxError::unsupported(
        "protocol.wire_version",
        "upgrade the rmux daemon or use an SDK that supports this wire version",
    );
    let rendered = error.to_string();
    let mut lines = rendered.lines();
    let head = lines.next().expect("display must produce a head line");
    let tail = lines.next().expect("display must produce a hint line");
    assert!(lines.next().is_none(), "display must be exactly two lines");

    assert_eq!(head, "unsupported feature `protocol.wire_version`");
    assert_eq!(
        tail,
        "hint: upgrade the rmux daemon or use an SDK that supports this wire version"
    );
}

#[test]
fn facade_error_accessors_expose_feature_and_hint() {
    let operation = "respawn";
    let error = RmuxError::unsupported(
        format!("operation.{operation}"),
        format!("the negotiated daemon does not support `{operation}` yet"),
    );
    assert_eq!(error.feature(), Some("operation.respawn"));
    assert_eq!(
        error.hint(),
        Some("the negotiated daemon does not support `respawn` yet")
    );
}

#[test]
fn facade_error_unsupported_has_no_underlying_source() {
    let error: RmuxError = RmuxError::unsupported("operation.attach", "leaf error");
    let as_std: &(dyn StdError + 'static) = &error;
    assert!(
        as_std.source().is_none(),
        "Unsupported is a leaf variant and must not expose a source"
    );
}
