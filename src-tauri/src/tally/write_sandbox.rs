//! Network-free controlled-write qualification.
//!
//! The implementation source is shared with the portable crate so its evidence
//! derivation can be tested without Tauri, SQLCipher, native libraries, or an
//! installed Tally. In this desktop module its sealed fields remain
//! crate-private; only the crate-private runtime coordinator can hand them to
//! the bounded loopback transport.

#[path = "../../crates/bridge-tally-write/src/lib.rs"]
#[allow(dead_code, unused_imports)]
mod implementation;

pub(crate) use implementation::*;
