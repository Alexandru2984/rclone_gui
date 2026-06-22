//! History: recent job runs read from SQLite. Call [`HistoryView::refresh`]
//! whenever the screen becomes visible.

use std::rc::Rc;

use adw::prelude::*;

use cascade_core::storage::RunRecord;

use crate::ctx::AppCtx;
use crate::views::job_details;

#[derive(Clone)]
pub struct HistoryView {
    root: gtk::Widget,
    list: gtk::ListBox,
    empty: gtk::Label,
    ctx: Rc<AppCtx>,
    window: adw::ApplicationWindow,
}

impl HistoryView {
    pub fn new(ctx: Rc<AppCtx>, window: adw::ApplicationWindow) -> Self {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list".to_string()])
            .build();

        let empty = gtk::Label::builder()
            .label(crate::i18n::tr(
                "No runs yet — start a job to see its history here.",
            ))
            .css_classes(vec!["dim-label".to_string()])
            .margin_top(24)
            .build();

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.append(&empty);
        column.append(&list);

        let clamp = adw::Clamp::builder()
            .maximum_size(720)
            .child(&column)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        scroller.set_margin_top(18);
        scroller.set_margin_bottom(18);
        scroller.set_margin_start(12);
        scroller.set_margin_end(12);

        Self {
            root: scroller.upcast(),
            list,
            empty,
            ctx,
            window,
        }
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.root
    }

    /// Reload rows from the store, newest first.
    pub fn refresh(&self) {
        self.list.remove_all();
        let runs = self.ctx.store.recent_runs(200).unwrap_or_default();
        self.empty.set_visible(runs.is_empty());
        self.list.set_visible(!runs.is_empty());
        for run in runs {
            let row = row_for(&run);
            row.set_activatable(true);
            let ctx = self.ctx.clone();
            let window = self.window.clone();
            row.connect_activated(move |_| {
                job_details::present(&window, &ctx, run.clone());
            });
            self.list.append(&row);
        }
    }
}

fn row_for(run: &RunRecord) -> adw::ActionRow {
    let mut title = format!("{} · {}", run.job_name, run.operation);
    if run.dry_run {
        title.push_str("  (dry-run)");
    }
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(&run.argv_preview)
        .build();
    row.add_prefix(&kind_icon(&run.kind));
    row.add_suffix(&status_pill(&run.status));
    if let Some(when) = run.started_at {
        let time = gtk::Label::builder()
            .label(relative_time(when))
            .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
            .build();
        row.add_suffix(&time);
    }
    row
}

fn kind_icon(kind: &str) -> gtk::Image {
    let name = match kind {
        "rclone" => "weather-overcast-symbolic", // cloud-ish
        _ => "folder-symbolic",
    };
    gtk::Image::from_icon_name(name)
}

fn status_pill(status: &str) -> gtk::Label {
    let css = match status {
        "completed" => "success",
        "failed" => "error",
        "running" => "accent",
        "cancelled" => "warning",
        _ => "dim-label",
    };
    gtk::Label::builder()
        .label(status)
        .css_classes(vec!["caption-heading".to_string(), css.to_string()])
        .build()
}

/// Compact "x min ago" style label from a unix timestamp.
fn relative_time(epoch: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(epoch);
    let secs = (now - epoch).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", secs / 60),
        3600..=86399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86400),
    }
}
