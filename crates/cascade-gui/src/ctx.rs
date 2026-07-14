//! Shared application context: the open SQLite store, resolved paths, and the
//! live application settings.
//!
//! GTK is single-threaded, so an `Rc<AppCtx>` is shared across all views. All
//! database access happens on the GLib main thread (process events are marshaled
//! back to it), so a non-`Send` `rusqlite::Connection` behind `Rc` is safe here.

use std::cell::RefCell;
use std::rc::Rc;

use tracing::warn;

use cascade_core::config::Paths;
use cascade_core::settings::{AppSettings, Theme};
use cascade_core::storage::Store;

pub struct AppCtx {
    pub store: Rc<Store>,
    pub paths: Paths,
    /// False when any application directory was unsafe (for example a
    /// symlink); filesystem persistence/logging must then stay disabled.
    pub paths_ready: bool,
    /// Live settings; mutated by the Settings screen, read by other views.
    pub settings: RefCell<AppSettings>,
}

impl AppCtx {
    pub fn new() -> Rc<Self> {
        let paths = Paths::resolve();
        let paths_ready = match paths.ensure() {
            Ok(()) => true,
            Err(e) => {
                warn!("unsafe or unavailable app directories ({e}); using memory-only state");
                false
            }
        };
        let store = match paths_ready {
            true => match Store::open(&paths.db_path) {
                Ok(store) => store,
                Err(e) => {
                    warn!("opening database failed ({e}); using an in-memory store");
                    Store::open_in_memory().expect("in-memory store")
                }
            },
            false => Store::open_in_memory().expect("in-memory store"),
        };
        // Clean up runs orphaned by a previous crash/hard exit.
        if let Err(e) = store.fail_interrupted_runs() {
            warn!("could not clean up interrupted runs: {e}");
        }
        match store.purge_embedded_secrets() {
            Ok(count) if count > 0 => {
                warn!("purged or redacted {count} legacy records containing credentials")
            }
            Ok(_) => {}
            Err(e) => warn!("could not purge legacy credentials from the database: {e}"),
        }
        if paths_ready {
            match cascade_core::logs::sanitize_existing_logs(&paths.log_dir) {
                Ok(count) if count > 0 => {
                    warn!("re-sanitized {count} historical log files containing credentials")
                }
                Ok(_) => {}
                Err(e) => warn!("could not re-sanitize historical logs: {e}"),
            }
            // Prune log files older than 30 days.
            let _ = cascade_core::logs::prune_logs_older_than_days(&paths.log_dir, 30);
        }
        let settings = AppSettings::load(&store);
        Rc::new(Self {
            store: Rc::new(store),
            paths,
            paths_ready,
            settings: RefCell::new(settings),
        })
    }

    /// Persist the current settings.
    pub fn save_settings(&self) {
        if let Err(e) = self.settings.borrow().save(&self.store) {
            warn!("could not save settings: {e}");
        }
    }
}

/// Apply a theme to the running application via libadwaita's style manager.
pub fn apply_theme(theme: Theme) {
    let scheme = match theme {
        Theme::System => adw::ColorScheme::Default,
        Theme::Light => adw::ColorScheme::ForceLight,
        Theme::Dark => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}
