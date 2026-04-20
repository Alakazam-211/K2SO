//! Scaffold test — verifies the session_stream feature gate compiles
//! in the three new modules and that they're reachable from downstream
//! code. Replaced by module-specific integration tests in C2-C6.

#![cfg(feature = "session_stream")]

use k2so_core::{awareness, session, term};

#[test]
fn modules_are_reachable() {
    assert_eq!(session::VERSION, "0.34.0-phase1");
    assert_eq!(awareness::VERSION, "0.34.0-phase1");
    assert_eq!(term::VERSION, "0.34.0-phase1");
}
