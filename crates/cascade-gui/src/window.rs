//! Main window: a view switcher over Dashboard / New Job / History.

use std::rc::Rc;

use adw::prelude::*;

use crate::ctx::AppCtx;
use crate::views::{dashboard, history::HistoryView, new_job};

pub struct MainWindow;

impl MainWindow {
    pub fn new(app: &adw::Application, ctx: Rc<AppCtx>) -> adw::ApplicationWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Cascade")
            .default_width(960)
            .default_height(700)
            .build();

        let stack = adw::ViewStack::new();

        // History view, refreshed whenever it becomes visible or a run finishes.
        let history = HistoryView::new(ctx.clone());
        let on_changed: Rc<dyn Fn()> = {
            let history = history.clone();
            Rc::new(move || history.refresh())
        };

        let new_job_page = new_job::build(ctx.clone(), window.clone(), on_changed);

        stack.add_titled_with_icon(&dashboard::build(), Some("dashboard"), "Dashboard", "go-home-symbolic");
        stack.add_titled_with_icon(&new_job_page, Some("new-job"), "New Job", "list-add-symbolic");
        stack.add_titled_with_icon(history.widget(), Some("history"), "History", "document-open-recent-symbolic");

        // Refresh History when the user switches to it.
        {
            let history = history.clone();
            stack.connect_visible_child_notify(move |s| {
                if s.visible_child_name().as_deref() == Some("history") {
                    history.refresh();
                }
            });
        }

        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();
        let header = adw::HeaderBar::builder().title_widget(&switcher).build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&stack));
        window.set_content(Some(&toolbar));

        window
    }
}
