//! Shared application context: the open SQLite store, resolved paths, and the
//! live application settings.
//!
//! GTK is single-threaded, so an `Rc<AppCtx>` is shared across all views. All
//! database access happens on the GLib main thread (process events are marshaled
//! back to it), so a non-`Send` `rusqlite::Connection` behind `Rc` is safe here.

use std::cell::RefCell;
use std::rc::Rc;

use cascade_core::config::Paths;
use cascade_core::settings::{AppSettings, Theme};
use cascade_core::storage::Store;

pub struct AppCtx {
    pub store: Rc<Store>,
    pub paths: Paths,
    /// Live settings; mutated by the Settings screen, read by other views.
    pub settings: RefCell<AppSettings>,
}

impl AppCtx {
    pub fn new() -> Rc<Self> {
        let paths = Paths::resolve();
        if let Err(e) = paths.ensure() {
            eprintln!("warning: could not create app directories: {e}");
        }
        let store = match Store::open(&paths.db_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: opening database failed ({e}); using in-memory store");
                Store::open_in_memory().expect("in-memory store")
            }
        };
        // Clean up runs orphaned by a previous crash/hard exit.
        if let Err(e) = store.fail_interrupted_runs() {
            eprintln!("warning: could not clean up interrupted runs: {e}");
        }
        // Prune log files older than 30 days.
        let _ = cascade_core::logs::prune_logs_older_than_days(&paths.log_dir, 30);
        let settings = AppSettings::load(&store);
        Rc::new(Self {
            store: Rc::new(store),
            paths,
            settings: RefCell::new(settings),
        })
    }

    /// Persist the current settings.
    pub fn save_settings(&self) {
        if let Err(e) = self.settings.borrow().save(&self.store) {
            eprintln!("warning: could not save settings: {e}");
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
