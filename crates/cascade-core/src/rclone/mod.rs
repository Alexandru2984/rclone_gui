//! rclone integration: binary detection and safe argv command building.
//!
//! Phase 2 will add the local `rcd` HTTP RC client; for the MVP we drive rclone
//! as a CLI process and parse `--use-json-log` / `--stats` output.

pub mod browse;
pub mod command;
pub mod detect;
pub mod mount;

pub use browse::Entry;
pub use command::{preview, RcloneOp, RcloneOptions};
pub use detect::{detect, ToolInfo};
pub use mount::MountOptions;
