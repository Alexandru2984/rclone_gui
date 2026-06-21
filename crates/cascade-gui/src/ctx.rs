//! Shared application context: the open SQLite store and resolved paths.
//!
//! GTK is single-threaded, so an `Rc<AppCtx>` is shared across all views. All
//! database access happens on the GLib main thread (process events are marshaled
//! back to it), so a non-`Send` `rusqlite::Connection` behind `Rc` is safe here.

use std::rc::Rc;

use cascade_core::config::Paths;
use cascade_core::storage::Store;

pub struct AppCtx {
    pub store: Rc<Store>,
    /// Resolved XDG paths; used by later phases for on-disk log files.
    #[allow(dead_code)]
    pub paths: Paths,
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
        Rc::new(Self { store: Rc::new(store), paths })
    }
}
