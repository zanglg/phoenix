#![no_std]
#![warn(missing_docs)]

//! Phoenix kernel crate.
//!
//! Version 0.0.0 intentionally contains no bootable kernel behavior. The crate
//! exists so that the AArch64 target and editor integration can be validated
//! before First Light development begins.

/// The Phoenix project version inherited from the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
