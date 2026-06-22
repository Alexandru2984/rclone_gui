//! Remote Browser: list rclone remotes and navigate their folders without
//! typing paths, then send the current location to New Job as source/dest.
//!
//! `rclone lsjson` runs off the UI thread via `process::capture`, so navigating
//! a slow cloud remote never freezes the interface.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use cascade_core::process::capture;
use cascade_core::rclone::browse;

use crate::ctx::AppCtx;
use crate::views::add_remote;

/// Where a picked path should go in the New Job form.
#[derive(Debug, Clone, Copy)]
pub enum PickTarget {
    Source,
    Destination,
}

struct State {
    /// Sub-path under the selected remote (no leading/trailing slash).
    sub: String,
}

#[derive(Clone)]
pub struct RemoteBrowserView {
    root: gtk::Widget,
    window: adw::ApplicationWindow,
    remote_combo: adw::ComboRow,
    path_label: gtk::Label,
    list: gtk::ListBox,
    status: gtk::Label,
    state: Rc<RefCell<State>>,
    remotes: Rc<RefCell<Vec<String>>>,
    on_pick: Rc<dyn Fn(PickTarget, String)>,
}

impl RemoteBrowserView {
    pub fn new(
        _ctx: Rc<AppCtx>,
        window: adw::ApplicationWindow,
        on_pick: Rc<dyn Fn(PickTarget, String)>,
    ) -> Self {
        let remote_combo = adw::ComboRow::builder().title("Remote").build();
        let remote_group = adw::PreferencesGroup::builder()
            .title("rclone remotes")
            .description("Pick a configured remote to browse")
            .build();
        remote_group.add(&remote_combo);
        let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
        add_btn.add_css_class("flat");
        add_btn.set_tooltip_text(Some("Add a new remote"));
        remote_group.set_header_suffix(Some(&add_btn));

        let up = gtk::Button::from_icon_name("go-up-symbolic");
        up.set_tooltip_text(Some("Go up one folder"));
        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_tooltip_text(Some("Reload"));
        let path_label = gtk::Label::builder()
            .xalign(0.0)
            .hexpand(true)
            .selectable(true)
            .css_classes(vec!["monospace".to_string()])
            .build();
        let nav = gtk::Box::builder().spacing(6).build();
        nav.append(&up);
        nav.append(&path_label);
        nav.append(&refresh);

        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(vec!["dim-label".to_string()])
            .build();

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list".to_string()])
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&list)
            .build();

        let use_src = gtk::Button::builder()
            .label("Use as Source")
            .css_classes(vec!["pill".to_string(), "suggested-action".to_string()])
            .build();
        let use_dst = gtk::Button::builder()
            .label("Use as Destination")
            .css_classes(vec!["pill".to_string()])
            .build();
        let pick = gtk::Box::builder()
            .halign(gtk::Align::End)
            .spacing(8)
            .build();
        pick.append(&use_src);
        pick.append(&use_dst);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_top(12);
        column.set_margin_bottom(12);
        column.set_margin_start(12);
        column.set_margin_end(12);
        column.append(&remote_group);
        column.append(&nav);
        column.append(&status);
        column.append(&scroller);
        column.append(&pick);

        let view = Self {
            root: column.upcast(),
            window,
            remote_combo,
            path_label,
            list,
            status,
            state: Rc::new(RefCell::new(State { sub: String::new() })),
            remotes: Rc::new(RefCell::new(Vec::new())),
            on_pick,
        };

        // Wire interactions.
        {
            let this = view.clone();
            view.remote_combo.connect_selected_notify(move |_| {
                this.state.borrow_mut().sub.clear();
                this.reload();
            });
        }
        {
            let this = view.clone();
            up.connect_clicked(move |_| {
                let parent = browse::parent_sub(&this.state.borrow().sub);
                this.state.borrow_mut().sub = parent;
                this.reload();
            });
        }
        {
            let this = view.clone();
            refresh.connect_clicked(move |_| this.reload());
        }
        {
            let this = view.clone();
            add_btn.connect_clicked(move |_| {
                let reload = this.clone();
                let on_done: Rc<dyn Fn()> = Rc::new(move || reload.load_remotes());
                add_remote::present(&this.window, on_done);
            });
        }
        {
            let this = view.clone();
            use_src.connect_clicked(move |_| this.pick(PickTarget::Source));
        }
        {
            let this = view.clone();
            use_dst.connect_clicked(move |_| this.pick(PickTarget::Destination));
        }

        view.load_remotes();
        view
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.root
    }

    fn current_remote(&self) -> Option<String> {
        let idx = self.remote_combo.selected() as usize;
        self.remotes.borrow().get(idx).cloned()
    }

    fn current_path(&self) -> Option<String> {
        let remote = self.current_remote()?;
        Some(browse::join(&remote, &self.state.borrow().sub))
    }

    fn pick(&self, target: PickTarget) {
        if let Some(path) = self.current_path() {
            (self.on_pick)(target, path);
        }
    }

    fn load_remotes(&self) {
        self.status.set_visible(true);
        self.status.set_label("Loading remotes…");
        let rx = capture("rclone", browse::listremotes_args());
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(out)) => {
                    let remotes = browse::parse_remotes(&out);
                    if remotes.is_empty() {
                        this.status.set_label(
                            "No rclone remotes configured. Run `rclone config` in a terminal to add one.",
                        );
                        return;
                    }
                    let names: Vec<&str> = remotes.iter().map(|s| s.as_str()).collect();
                    let model = gtk::StringList::new(&names);
                    // Populate the backing vec before setting the model, so any
                    // `selected_notify` fired during set_model sees the remotes.
                    *this.remotes.borrow_mut() = remotes.clone();
                    this.remote_combo.set_model(Some(&model));
                    this.remote_combo.set_selected(0);
                    this.state.borrow_mut().sub.clear();
                    this.reload();
                }
                Ok(Err(e)) => this.status.set_label(&e),
                Err(_) => {}
            }
        });
    }

    fn reload(&self) {
        let Some(path) = self.current_path() else {
            return;
        };
        self.path_label.set_label(&path);
        self.status.set_visible(true);
        self.status.set_label(&format!("Loading {path} …"));
        self.list.remove_all();

        let rx = capture("rclone", browse::lsjson_args(&path));
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(stdout)) => match browse::parse_lsjson(&stdout) {
                    Ok(entries) => this.populate(entries),
                    Err(e) => this
                        .status
                        .set_label(&format!("Could not read listing: {e}")),
                },
                Ok(Err(e)) => this.status.set_label(&e),
                Err(_) => this.status.set_label("Listing cancelled"),
            }
        });
    }

    fn populate(&self, entries: Vec<browse::Entry>) {
        if entries.is_empty() {
            self.status.set_label("Empty folder");
            self.status.set_visible(true);
            return;
        }
        self.status.set_visible(false);
        for entry in entries {
            let subtitle = if entry.is_dir {
                "folder".to_string()
            } else {
                fmt_size(entry.size)
            };
            let row = adw::ActionRow::builder()
                .title(&entry.name)
                .subtitle(&subtitle)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(if entry.is_dir {
                "folder-symbolic"
            } else {
                "text-x-generic-symbolic"
            }));

            if entry.is_dir {
                row.set_activatable(true);
                let this = self.clone();
                let name = entry.name.clone();
                row.connect_activated(move |_| {
                    let mut s = this.state.borrow_mut();
                    s.sub = if s.sub.is_empty() {
                        name.clone()
                    } else {
                        format!("{}/{}", s.sub, name)
                    };
                    drop(s);
                    this.reload();
                });
            }
            self.list.append(&row);
        }
    }
}

/// Human-readable size, or `—` when unknown (rclone reports -1 for some dirs).
fn fmt_size(bytes: i64) -> String {
    if bytes < 0 {
        return "—".to_string();
    }
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}
