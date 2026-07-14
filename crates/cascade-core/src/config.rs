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
            if !std::fs::symlink_metadata(d)?.file_type().is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "application directory '{}' must not be a symlink",
                        d.display()
                    ),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_has_sensible_layout() {
        let p = Paths::resolve();
        // The database lives under the data dir and is named cascade.db.
        assert_eq!(p.db_path.file_name().unwrap(), "cascade.db");
        assert!(p.db_path.starts_with(&p.data_dir));
        // Logs live under the data dir.
        assert!(p.log_dir.starts_with(&p.data_dir));
        assert_eq!(p.log_dir.file_name().unwrap(), "logs");
        // resolve() must not create anything on disk by itself.
    }

    #[test]
    fn ensure_creates_private_directories() {
        // Point Paths at a throwaway root so we never touch the real XDG dirs.
        let root = tempfile::tempdir().unwrap();
        let base = root.path();
        let paths = Paths {
            config_dir: base.join("config"),
            data_dir: base.join("data"),
            log_dir: base.join("data/logs"),
            db_path: base.join("data/cascade.db"),
        };
        paths.ensure().unwrap();

        for d in [&paths.config_dir, &paths.data_dir, &paths.log_dir] {
            assert!(d.is_dir(), "{d:?} was not created");
        }
        // ensure() is idempotent.
        paths.ensure().unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&paths.data_dir)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "data dir should be private (0700)");
        }
    }

    #[cfg(unix)]
    #[test]
    fn ensure_refuses_symlinked_application_directories() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = root.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let data = root.path().join("data-link");
        symlink(&outside, &data).unwrap();
        let paths = Paths {
            config_dir: root.path().join("config"),
            data_dir: data.clone(),
            log_dir: data.join("logs"),
            db_path: data.join("cascade.db"),
        };
        assert!(paths.ensure().is_err());
    }
}
