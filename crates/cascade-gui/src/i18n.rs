//! Internationalization via gettext.
//!
//! User-facing strings are wrapped in [`tr`]. At startup [`init`] binds the
//! `cascade` text domain to a locale directory (the installed
//! `/usr/share/locale`, or `$CASCADE_LOCALE_DIR` for development). With no
//! catalog installed, strings pass through unchanged (English).

use gettextrs::{bind_textdomain_codeset, bindtextdomain, setlocale, textdomain, LocaleCategory};

pub const DOMAIN: &str = "cascade";

/// Translate a message id for the current locale.
pub fn tr(msgid: &str) -> String {
    gettextrs::gettext(msgid)
}

/// Honor the user's locale environment and bind our text domain.
pub fn init() {
    let _ = setlocale(LocaleCategory::LcAll, "");
    let _ = bindtextdomain(DOMAIN, locale_dir());
    let _ = bind_textdomain_codeset(DOMAIN, "UTF-8");
    let _ = textdomain(DOMAIN);
}

fn locale_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("CASCADE_LOCALE_DIR") {
        if !dir.is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    std::path::PathBuf::from("/usr/share/locale")
}
