//! Application paths and configuration (XDG-compliant).

use std::path::PathBuf;

use directories::ProjectDirs;

/// Reverse-DNS application id — also used as the GTK/libadwaita app id and the
/// desktop file name. Change this if you rename the project.
pub const APP_ID: &str = "io.github.alexmihai.Cascade";
pub const APP_NAME: &str = "Cascade";

/// Resolved on-disk locations for config, data, and logs.
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    pub db_path: PathBuf,
}

impl Paths {
    /// Resolve standard XDG locations for the app.
    pub fn resolve() -> Self {
        let dirs = ProjectDirs::from("io.github", "alexmihai", APP_NAME)
            .expect("a home directory must exist");
        let config_dir = dirs.config_dir().to_path_buf();
        let data_dir = dirs.data_dir().to_path_buf();
        let log_dir = data_dir.join("logs");
        let db_path = data_dir.join("cascade.db");
        Self {
            config_dir,
            data_dir,
            log_dir,
            db_path,
        }
    }

    /// Create the directories with private (0700) permissions on Unix.
    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [&self.config_dir, &self.data_dir, &self.log_dir] {
            std::fs::create_dir_all(d)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }
}
