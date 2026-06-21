//! Settings: theme, confirmation behavior, and parallelism — persisted live.

use std::rc::Rc;

use adw::prelude::*;

use cascade_core::settings::Theme;

use crate::ctx::{apply_theme, AppCtx};

pub fn build(ctx: Rc<AppCtx>) -> gtk::Widget {
    let appearance = adw::PreferencesGroup::builder().title("Appearance").build();

    let theme_row = adw::ComboRow::builder().title("Theme").build();
    theme_row.set_model(Some(&gtk::StringList::new(&[
        "Follow system",
        "Light",
        "Dark",
    ])));
    theme_row.set_selected(match ctx.settings.borrow().theme {
        Theme::System => 0,
        Theme::Light => 1,
        Theme::Dark => 2,
    });
    {
        let ctx = ctx.clone();
        theme_row.connect_selected_notify(move |row| {
            let theme = match row.selected() {
                1 => Theme::Light,
                2 => Theme::Dark,
                _ => Theme::System,
            };
            ctx.settings.borrow_mut().theme = theme;
            apply_theme(theme);
            ctx.save_settings();
        });
    }
    appearance.add(&theme_row);

    let behavior = adw::PreferencesGroup::builder().title("Behavior").build();

    let confirm_row = adw::SwitchRow::builder()
        .title("Confirm destructive operations")
        .subtitle("Ask before running sync-with-delete, delete, or purge")
        .active(ctx.settings.borrow().confirm_destructive)
        .build();
    {
        let ctx = ctx.clone();
        confirm_row.connect_active_notify(move |row| {
            ctx.settings.borrow_mut().confirm_destructive = row.is_active();
            ctx.save_settings();
        });
    }
    behavior.add(&confirm_row);

    let parallel_row = adw::SpinRow::builder()
        .title("Maximum parallel jobs")
        .subtitle("Used by the job queue")
        .adjustment(&gtk::Adjustment::new(
            ctx.settings.borrow().max_parallel as f64,
            1.0,
            8.0,
            1.0,
            1.0,
            0.0,
        ))
        .build();
    {
        let ctx = ctx.clone();
        parallel_row.connect_value_notify(move |row| {
            ctx.settings.borrow_mut().max_parallel = row.value() as u32;
            ctx.save_settings();
        });
    }
    behavior.add(&parallel_row);

    let page = adw::PreferencesPage::new();
    page.add(&appearance);
    page.add(&behavior);
    page.upcast()
}
