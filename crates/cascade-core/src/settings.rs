//! Typed application settings, persisted as a single JSON row in `settings`.

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::storage::Store;

const KEY: &str = "app_settings";
pub const MAX_PARALLEL_JOBS: u32 = 8;

/// Color scheme preference (maps to libadwaita's color scheme in the GUI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
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
        let mut settings: Self = store
            .get_setting(KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        settings.max_parallel = settings.max_parallel.clamp(1, MAX_PARALLEL_JOBS);
        settings
    }

    /// Persist the settings as JSON.
    pub fn save(&self, store: &Store) -> Result<()> {
        let mut safe = self.clone();
        safe.max_parallel = safe.max_parallel.clamp(1, MAX_PARALLEL_JOBS);
        let json = serde_json::to_string(&safe)?;
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

    #[test]
    fn persisted_parallelism_is_clamped() {
        let store = Store::open_in_memory().unwrap();
        store
            .set_setting(
                KEY,
                r#"{"theme":"system","max_parallel":4294967295,"confirm_destructive":true}"#,
            )
            .unwrap();
        assert_eq!(AppSettings::load(&store).max_parallel, MAX_PARALLEL_JOBS);

        let settings = AppSettings {
            max_parallel: 0,
            ..Default::default()
        };
        settings.save(&store).unwrap();
        assert_eq!(AppSettings::load(&store).max_parallel, 1);
    }
}
