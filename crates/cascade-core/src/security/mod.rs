//! Security primitives shared by every operation.
//!
//! Three concerns, each its own module and test suite:
//! - [`path`]: reject catastrophic source/destination selections.
//! - [`destructive`]: classify how dangerous an operation is.
//! - [`sanitize`]: redact secrets from log output before display or storage.

pub mod destructive;
pub mod path;
pub mod sanitize;
