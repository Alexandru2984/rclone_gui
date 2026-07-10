//! Screen widgets. Each view builds a self-contained GTK widget tree and wires
//! its own behavior against the shared [`crate::ctx::AppCtx`].

/// Escape text for an Adw/GTK label that interprets Pango markup (row titles and
/// subtitles do by default). A filename or path containing `&`, `<` or `>`
/// would otherwise fail markup parsing and render nothing.
pub fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

pub mod add_remote;
pub mod assistant;
pub mod dashboard;
pub mod history;
pub mod job_details;
pub mod mounts;
pub mod new_job;
pub mod profiles;
pub mod queue;
pub mod remote_browser;
pub mod schedule;
pub mod settings;
