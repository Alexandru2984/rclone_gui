//! Main window: a view switcher over Dashboard / New Job / Profiles / History /
//! Settings, with cross-view wiring (load a profile into New Job; refresh
//! History and Profiles after changes).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use cascade_core::job::JobSpec;

use crate::ctx::AppCtx;
use crate::views::{dashboard, history::HistoryView, new_job, profiles::ProfilesView, settings};

pub struct MainWindow;

impl MainWindow {
    pub fn new(app: &adw::Application, ctx: Rc<AppCtx>) -> adw::ApplicationWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Cascade")
            .default_width(980)
            .default_height(720)
            .build();

        let stack = adw::ViewStack::new();

        // History + a slot for Profiles so `on_changed` can refresh both even
        // though Profiles is constructed slightly later.
        let history = HistoryView::new(ctx.clone());
        let profiles_slot: Rc<RefCell<Option<ProfilesView>>> = Rc::new(RefCell::new(None));

        let on_changed: Rc<dyn Fn()> = {
            let history = history.clone();
            let profiles_slot = profiles_slot.clone();
            Rc::new(move || {
                history.refresh();
                if let Some(p) = profiles_slot.borrow().as_ref() {
                    p.refresh();
                }
            })
        };

        let new_job = Rc::new(new_job::build(ctx.clone(), window.clone(), on_changed));

        // Loading a profile fills the New Job form and switches to it.
        let on_load: Rc<dyn Fn(JobSpec)> = {
            let new_job = new_job.clone();
            let stack = stack.clone();
            Rc::new(move |spec| {
                new_job.load_spec(&spec);
                stack.set_visible_child_name("new-job");
            })
        };
        let profiles = ProfilesView::new(ctx.clone(), on_load);
        *profiles_slot.borrow_mut() = Some(profiles.clone());

        stack.add_titled_with_icon(
            &dashboard::build(),
            Some("dashboard"),
            "Dashboard",
            "go-home-symbolic",
        );
        stack.add_titled_with_icon(
            new_job.widget(),
            Some("new-job"),
            "New Job",
            "list-add-symbolic",
        );
        stack.add_titled_with_icon(
            profiles.widget(),
            Some("profiles"),
            "Profiles",
            "starred-symbolic",
        );
        stack.add_titled_with_icon(
            history.widget(),
            Some("history"),
            "History",
            "document-open-recent-symbolic",
        );
        stack.add_titled_with_icon(
            &settings::build(ctx.clone()),
            Some("settings"),
            "Settings",
            "emblem-system-symbolic",
        );

        // Refresh list views when the user navigates to them.
        {
            let history = history.clone();
            let profiles = profiles.clone();
            stack.connect_visible_child_notify(move |s| match s.visible_child_name().as_deref() {
                Some("history") => history.refresh(),
                Some("profiles") => profiles.refresh(),
                _ => {}
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
