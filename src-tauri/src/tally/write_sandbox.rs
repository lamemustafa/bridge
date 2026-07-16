//! Network-free controlled-write qualification.
//!
//! The implementation lives in a portable crate so its evidence derivation can
//! be tested without Tauri, SQLCipher, native libraries, or an installed Tally.
//! It intentionally exposes no HTTP dispatch adapter.

pub use bridge_tally_write::*;
