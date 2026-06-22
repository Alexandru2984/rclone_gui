//! Main window: a view switcher over Dashboard / New Job / Profiles / History /
//! Settings, with cross-view wiring (load a profile into New Job; refresh
//! History and Profiles after changes).

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use cascade_core::job::JobSpec;

use crate::ctx::AppCtx;
use crate::views::mounts::MountsView;
use crate::views::queue::QueueView;
use crate::views::remote_browser::{PickTarget, RemoteBrowserView};
use crate::views::{
    assistant, dashboard, history::HistoryView, new_job, profiles::ProfilesView, settings,
};

pub struct MainWindow;

impl MainWindow {
    pub fn build(app: &adw::Application, ctx: Rc<AppCtx>) -> adw::ApplicationWindow {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Cascade")
            .default_width(980)
            .default_height(720)
            .build();

        let stack = adw::ViewStack::new();

        // History + a slot for Profiles so `on_changed` can refresh both even
        // though Profiles is constructed slightly later.
        let history = HistoryView::new(ctx.clone(), window.clone());
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

        // Jobs queue: runs up to max_parallel jobs; refreshes History on change.
        let queue = QueueView::new(ctx.clone(), on_changed.clone());
        let on_enqueue: Rc<dyn Fn(JobSpec)> = {
            let queue = queue.clone();
            let stack = stack.clone();
            Rc::new(move |spec| {
                queue.enqueue(spec);
                stack.set_visible_child_name("queue");
            })
        };

        let new_job = Rc::new(new_job::build(
            ctx.clone(),
            window.clone(),
            on_changed,
            on_enqueue,
        ));

        // Loading a profile fills the New Job form and switches to it.
        let on_load: Rc<dyn Fn(JobSpec)> = {
            let new_job = new_job.clone();
            let stack = stack.clone();
            Rc::new(move |spec| {
                new_job.load_spec(&spec);
                stack.set_visible_child_name("new-job");
            })
        };
        // The Assistant produces a preconfigured spec and reuses the same
        // "load into New Job and switch there" path as profiles.
        let assistant_widget = assistant::build(window.clone(), on_load.clone());

        let profiles = ProfilesView::new(ctx.clone(), on_load);
        *profiles_slot.borrow_mut() = Some(profiles.clone());

        // Remote browser: picking a path fills New Job and switches to it.
        let on_pick: Rc<dyn Fn(PickTarget, String)> = {
            let new_job = new_job.clone();
            let stack = stack.clone();
            Rc::new(move |target, path| {
                match target {
                    PickTarget::Source => new_job.set_source(&path),
                    PickTarget::Destination => new_job.set_destination(&path),
                }
                stack.set_visible_child_name("new-job");
            })
        };
        let remotes = RemoteBrowserView::new(ctx.clone(), window.clone(), on_pick);
        let mounts = MountsView::new(ctx.clone(), window.clone());

        stack.add_titled_with_icon(
            &dashboard::build(),
            Some("dashboard"),
            "Dashboard",
            "go-home-symbolic",
        );
        stack.add_titled_with_icon(
            &assistant_widget,
            Some("assistant"),
            "Assistant",
            "starred-symbolic",
        );
        stack.add_titled_with_icon(
            new_job.widget(),
            Some("new-job"),
            "New Job",
            "list-add-symbolic",
        );
        stack.add_titled_with_icon(
            remotes.widget(),
            Some("remotes"),
            "Remotes",
            "network-server-symbolic",
        );
        stack.add_titled_with_icon(
            mounts.widget(),
            Some("mounts"),
            "Mounts",
            "drive-harddisk-symbolic",
        );
        stack.add_titled_with_icon(queue.widget(), Some("queue"), "Queue", "view-list-symbolic");
        stack.add_titled_with_icon(
            profiles.widget(),
            Some("profiles"),
            "Profiles",
            "user-bookmarks-symbolic",
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
        if let Some(banner) = missing_tools_banner() {
            toolbar.add_top_bar(&banner);
        }
        toolbar.set_content(Some(&stack));
        window.set_content(Some(&toolbar));

        window
    }
}

/// A warning banner if rclone/rsync are not on PATH, so users aren't surprised
/// by jobs failing with exit status 127.
fn missing_tools_banner() -> Option<adw::Banner> {
    let mut missing = Vec::new();
    if cascade_core::rsync::detect().is_none() {
        missing.push("rsync");
    }
    if cascade_core::rclone::detect().is_none() {
        missing.push("rclone");
    }
    if missing.is_empty() {
        return None;
    }
    let banner = adw::Banner::new(&format!(
        "{} not found on PATH — related jobs will fail until it is installed.",
        missing.join(" and ")
    ));
    banner.set_revealed(true);
    Some(banner)
}
