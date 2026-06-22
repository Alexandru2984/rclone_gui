//! Job Details: a dialog showing one run's metadata, its exact command (with a
//! "Copy command" button), and its on-disk log filtered by severity.

use std::rc::Rc;

use adw::prelude::*;

use cascade_core::logs::{classify, Level};
use cascade_core::storage::RunRecord;

use crate::ctx::AppCtx;

/// Show the details dialog for `run`, attached to `parent`.
pub fn present(parent: &adw::ApplicationWindow, ctx: &Rc<AppCtx>, run: RunRecord) {
    let dialog = adw::Dialog::new();
    dialog.set_title(&crate::i18n::tr("Run details"));
    dialog.set_content_width(700);
    dialog.set_content_height(600);

    // --- Metadata ---
    let info = adw::PreferencesGroup::new();
    info.add(&detail_row(&crate::i18n::tr("Status"), &run.status));
    info.add(&detail_row(
        "Operation",
        &format!(
            "{} · {}{}",
            run.kind,
            run.operation,
            if run.dry_run { " · dry-run" } else { "" }
        ),
    ));
    info.add(&detail_row(
        "Exit code",
        &run.exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "—".into()),
    ));
    info.add(&detail_row(
        &crate::i18n::tr("Started"),
        &fmt_epoch(run.started_at),
    ));
    info.add(&detail_row(
        &crate::i18n::tr("Ended"),
        &fmt_epoch(run.ended_at),
    ));

    let cmd_row = adw::ActionRow::builder()
        .title(crate::i18n::tr("Command"))
        .subtitle(&run.argv_preview)
        .build();
    cmd_row.add_css_class("property");
    let copy = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(vec!["flat".to_string()])
        .tooltip_text("Copy command")
        .build();
    {
        let cmd = run.argv_preview.clone();
        copy.connect_clicked(move |b| {
            b.clipboard().set_text(&cmd);
        });
    }
    cmd_row.add_suffix(&copy);
    info.add(&cmd_row);

    // --- Log viewer with severity filters ---
    let lines: Rc<Vec<String>> = Rc::new(load_log(ctx, run.run_id));

    let view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .build();
    let buffer = view.buffer();
    let scroller = gtk::ScrolledWindow::builder()
        .min_content_height(260)
        .vexpand(true)
        .child(&view)
        .build();

    let apply: Rc<dyn Fn(Option<Level>)> = {
        let lines = lines.clone();
        let buffer = buffer.clone();
        Rc::new(move |filter: Option<Level>| {
            let text = lines
                .iter()
                .filter(|l| match filter {
                    None => true,
                    Some(lv) => classify(l) == lv,
                })
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            buffer.set_text(&text);
        })
    };

    let all = gtk::ToggleButton::with_label(&crate::i18n::tr("All"));
    all.set_active(true);
    let errors = gtk::ToggleButton::with_label(&crate::i18n::tr("Errors"));
    let warnings = gtk::ToggleButton::with_label(&crate::i18n::tr("Warnings"));
    let infos = gtk::ToggleButton::with_label(&crate::i18n::tr("Info"));
    for b in [&errors, &warnings, &infos] {
        b.set_group(Some(&all));
    }
    let filters = gtk::Box::builder()
        .spacing(0)
        .css_classes(vec!["linked".to_string()])
        .build();
    for b in [&all, &errors, &warnings, &infos] {
        filters.append(b);
    }

    macro_rules! on_filter {
        ($btn:expr, $val:expr) => {{
            let apply = apply.clone();
            $btn.connect_toggled(move |b| {
                if b.is_active() {
                    apply($val);
                }
            });
        }};
    }
    on_filter!(all, None);
    on_filter!(errors, Some(Level::Error));
    on_filter!(warnings, Some(Level::Warning));
    on_filter!(infos, Some(Level::Info));
    apply(None);

    let log_group = adw::PreferencesGroup::builder()
        .title(crate::i18n::tr("Log"))
        .build();
    log_group.add(&filters);

    // --- Assemble ---
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.append(&info);
    content.append(&log_group);
    content.append(&scroller);

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));

    dialog.set_child(Some(&toolbar));
    dialog.present(Some(parent));
}

fn detail_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(value)
        .build();
    row.add_css_class("property");
    row
}

fn load_log(ctx: &Rc<AppCtx>, run_id: i64) -> Vec<String> {
    match ctx.store.run_log_path(run_id) {
        Ok(Some(path)) => match std::fs::read_to_string(&path) {
            Ok(text) => text.lines().map(|s| s.to_string()).collect(),
            Err(_) => vec![format!("(log file is missing: {path})")],
        },
        _ => vec!["(no log recorded for this run)".to_string()],
    }
}

fn fmt_epoch(epoch: Option<i64>) -> String {
    match epoch {
        None => "—".to_string(),
        Some(secs) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(secs);
            let ago = (now - secs).max(0);
            match ago {
                0..=59 => "just now".to_string(),
                60..=3599 => format!("{} min ago", ago / 60),
                3600..=86399 => format!("{} h ago", ago / 3600),
                _ => format!("{} days ago", ago / 86400),
            }
        }
    }
}
