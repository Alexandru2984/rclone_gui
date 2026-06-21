//! rsync integration: binary detection and safe argv command building.

pub mod command;
pub mod detect;

pub use command::{build_args, RsyncOptions};
pub use detect::detect;
