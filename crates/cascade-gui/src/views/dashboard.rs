//! Dashboard: detection status for the external tools.

use adw::prelude::*;

pub fn build() -> gtk::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Tools")
        .description("Cascade orchestrates these external programs")
        .build();

    group.add(&tool_row(
        "rclone",
        cascade_core::rclone::detect().map(|i| i.version),
        "not installed — install it to enable cloud remotes",
    ));
    group.add(&tool_row(
        "rsync",
        cascade_core::rsync::detect().map(|i| i.version),
        "not installed",
    ));

    let page = adw::PreferencesPage::new();
    page.add(&group);
    page.upcast()
}

fn tool_row(name: &str, version: Option<String>, missing_hint: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(name).build();
    match version {
        Some(v) => {
            row.set_subtitle(&v);
            row.add_suffix(&status_icon(true));
        }
        None => {
            row.set_subtitle(missing_hint);
            row.add_suffix(&status_icon(false));
        }
    }
    row
}

fn status_icon(ok: bool) -> gtk::Image {
    let name = if ok {
        "emblem-ok-symbolic"
    } else {
        "dialog-warning-symbolic"
    };
    let img = gtk::Image::from_icon_name(name);
    img.add_css_class(if ok { "success" } else { "warning" });
    img
}
