#![no_std]
#![warn(missing_docs)]

//! Phoenix kernel crate.
//!
//! The library contains target-independent mechanisms where practical and
//! platform-specific modules behind explicit architecture paths.

pub mod build_info;
pub mod console;
pub mod dtb;
pub mod memory;

#[cfg(target_arch = "aarch64")]
pub mod arch;

pub mod platform;

/// The Phoenix project version inherited from the workspace manifest.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
