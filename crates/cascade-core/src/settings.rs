//! Typed application settings, persisted as a single JSON row in `settings`.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::storage::Store;

const KEY: &str = "app_settings";

/// Color scheme preference (maps to libadwaita's color scheme in the GUI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::System
    }
}

/// User-configurable application settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme: Theme,
    /// Maximum number of jobs allowed to run in parallel (used by the queue).
    pub max_parallel: u32,
    /// Whether destructive operations require an explicit confirmation dialog.
    pub confirm_destructive: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            max_parallel: 2,
            confirm_destructive: true,
        }
    }
}

impl AppSettings {
    /// Load settings from the store, falling back to defaults on absence/parse error.
    pub fn load(store: &Store) -> Self {
        store
            .get_setting(KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    /// Persist the settings as JSON.
    pub fn save(&self, store: &Store) -> Result<()> {
        let json = serde_json::to_string(self)?;
        store.set_setting(KEY, &json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        let store = Store::open_in_memory().unwrap();
        let s = AppSettings::load(&store);
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.max_parallel, 2);
        assert!(s.confirm_destructive);
    }

    #[test]
    fn roundtrip_through_store() {
        let store = Store::open_in_memory().unwrap();
        let s = AppSettings {
            theme: Theme::Dark,
            max_parallel: 4,
            confirm_destructive: false,
        };
        s.save(&store).unwrap();
        let back = AppSettings::load(&store);
        assert_eq!(back.theme, Theme::Dark);
        assert_eq!(back.max_parallel, 4);
        assert!(!back.confirm_destructive);
    }
}
