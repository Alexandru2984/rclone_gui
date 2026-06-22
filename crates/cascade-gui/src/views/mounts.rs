//! Mounts: mount an rclone remote onto a local folder and manage active mounts.
//!
//! A mount is a long-running `rclone mount` process kept alive in `active`.
//! Unmounting runs `fusermount -u <mountpoint>`, which makes that process exit
//! cleanly; its `Finished` event then removes the row.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::gio;

use cascade_core::process::{capture, spawn, ProcessEvent, RunHandle};
use cascade_core::rclone::browse;
use cascade_core::rclone::mount::{mount_args, unmount_args, MountOptions, UNMOUNT_BIN};

use crate::ctx::AppCtx;

struct ActiveMount {
    id: u64,
    handle: RunHandle,
    row: adw::ActionRow,
}

#[derive(Clone)]
pub struct MountsView {
    root: gtk::Widget,
    window: adw::ApplicationWindow,
    remote_combo: adw::ComboRow,
    subpath: adw::EntryRow,
    mountpoint: adw::EntryRow,
    read_only: adw::SwitchRow,
    cache: adw::SwitchRow,
    status: gtk::Label,
    active_list: gtk::ListBox,
    active_empty: gtk::Label,
    remotes: Rc<RefCell<Vec<String>>>,
    active: Rc<RefCell<Vec<ActiveMount>>>,
    next_id: Rc<Cell<u64>>,
}

impl MountsView {
    pub fn new(_ctx: Rc<AppCtx>, window: adw::ApplicationWindow) -> Self {
        let remote_combo = adw::ComboRow::builder()
            .title(crate::i18n::tr("Remote"))
            .build();
        let subpath = adw::EntryRow::builder()
            .title(crate::i18n::tr("Sub-path (optional)"))
            .build();
        let mountpoint = adw::EntryRow::builder()
            .title(crate::i18n::tr("Mountpoint (an existing folder)"))
            .build();

        let create_group = adw::PreferencesGroup::builder()
            .title(crate::i18n::tr("New mount"))
            .description(crate::i18n::tr("Mount a remote onto a local folder"))
            .build();
        create_group.add(&remote_combo);
        create_group.add(&subpath);
        create_group.add(&mountpoint);

        let read_only = adw::SwitchRow::builder()
            .title(crate::i18n::tr("Read-only"))
            .build();
        let cache = adw::SwitchRow::builder()
            .title(crate::i18n::tr("Writeback cache"))
            .subtitle(crate::i18n::tr(
                "--vfs-cache-mode writes (recommended for editing files)",
            ))
            .active(true)
            .build();
        let opts_group = adw::PreferencesGroup::builder()
            .title(crate::i18n::tr("Options"))
            .build();
        opts_group.add(&read_only);
        opts_group.add(&cache);

        let mount_btn = gtk::Button::builder()
            .label(crate::i18n::tr("Mount"))
            .css_classes(vec!["pill".to_string(), "suggested-action".to_string()])
            .build();
        let btn_box = gtk::Box::builder().halign(gtk::Align::End).build();
        btn_box.append(&mount_btn);

        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(vec!["dim-label".to_string()])
            .build();

        let active_list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list".to_string()])
            .build();
        let active_empty = gtk::Label::builder()
            .label(crate::i18n::tr("No active mounts."))
            .xalign(0.0)
            .css_classes(vec!["dim-label".to_string()])
            .build();
        let active_group = adw::PreferencesGroup::builder()
            .title(crate::i18n::tr("Active mounts"))
            .build();
        active_group.add(&active_empty);
        active_group.add(&active_list);

        let page = adw::PreferencesPage::new();
        page.add(&create_group);
        page.add(&opts_group);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_top(4);
        column.set_margin_bottom(12);
        column.set_margin_start(12);
        column.set_margin_end(12);
        column.append(&page);
        column.append(&btn_box);
        column.append(&status);

        // Active list lives in its own scrollable area below.
        let active_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        active_box.set_margin_start(12);
        active_box.set_margin_end(12);
        active_box.set_margin_bottom(12);
        active_box.append(&active_group);
        column.append(&active_box);

        let scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&column)
            .build();

        let view = Self {
            root: scroller.upcast(),
            window,
            remote_combo,
            subpath,
            mountpoint,
            read_only,
            cache,
            status,
            active_list,
            active_empty,
            remotes: Rc::new(RefCell::new(Vec::new())),
            active: Rc::new(RefCell::new(Vec::new())),
            next_id: Rc::new(Cell::new(1)),
        };

        // Folder picker for the mountpoint.
        let browse_btn = gtk::Button::from_icon_name("folder-open-symbolic");
        browse_btn.add_css_class("flat");
        browse_btn.set_valign(gtk::Align::Center);
        view.mountpoint.add_suffix(&browse_btn);
        {
            let this = view.clone();
            browse_btn.connect_clicked(move |_| this.pick_mountpoint());
        }
        {
            let this = view.clone();
            mount_btn.connect_clicked(move |_| this.do_mount());
        }

        view.refresh_active_visibility();
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

    fn pick_mountpoint(&self) {
        let dialog = gtk::FileDialog::builder()
            .title(crate::i18n::tr("Select mountpoint folder"))
            .build();
        let entry = self.mountpoint.clone();
        dialog.select_folder(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Ok(file) = res {
                if let Some(p) = file.path() {
                    entry.set_text(&p.to_string_lossy());
                }
            }
        });
    }

    fn load_remotes(&self) {
        let rx = capture("rclone", browse::listremotes_args());
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                Ok(Ok(out)) => {
                    let remotes = browse::parse_remotes(&out);
                    if remotes.is_empty() {
                        this.status.set_label(&crate::i18n::tr(
                            "No rclone remotes configured. Run `rclone config`.",
                        ));
                        return;
                    }
                    let names: Vec<&str> = remotes.iter().map(|s| s.as_str()).collect();
                    let model = gtk::StringList::new(&names);
                    *this.remotes.borrow_mut() = remotes;
                    this.remote_combo.set_model(Some(&model));
                    this.remote_combo.set_selected(0);
                }
                Ok(Err(e)) => this.status.set_label(&e),
                Err(_) => {}
            }
        });
    }

    fn do_mount(&self) {
        let Some(remote) = self.current_remote() else {
            self.status
                .set_label(&crate::i18n::tr("Select a remote first."));
            return;
        };
        let path = browse::join(&remote, &self.subpath.text());
        let mp = self.mountpoint.text().trim().to_string();
        if mp.is_empty() || !Path::new(&mp).is_dir() {
            self.status
                .set_label(&crate::i18n::tr("Mountpoint must be an existing folder."));
            return;
        }

        let opts = MountOptions {
            read_only: self.read_only.is_active(),
            vfs_cache_writes: self.cache.is_active(),
        };
        let argv = match mount_args(&path, &mp, &opts) {
            Ok(a) => a,
            Err(e) => {
                self.status.set_label(&format!("{e}"));
                return;
            }
        };
        self.status.set_label(&crate::i18n::tr(""));

        let handle = spawn("rclone", argv);
        let events = handle.events.clone();

        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let row = adw::ActionRow::builder()
            .title(format!("{path}  →  {mp}"))
            .subtitle(crate::i18n::tr("mounting…"))
            .build();
        row.add_prefix(&gtk::Image::from_icon_name("folder-remote-symbolic"));
        let unmount = gtk::Button::builder()
            .label(crate::i18n::tr("Unmount"))
            .valign(gtk::Align::Center)
            .css_classes(vec!["flat".to_string()])
            .build();
        {
            let this = self.clone();
            let mp = mp.clone();
            unmount.connect_clicked(move |_| this.unmount(id, &mp));
        }
        row.add_suffix(&unmount);

        self.active_list.append(&row);
        self.active.borrow_mut().push(ActiveMount {
            id,
            handle,
            row: row.clone(),
        });
        self.refresh_active_visibility();

        // If the process is still alive after a moment, consider it mounted.
        let exited = Rc::new(Cell::new(false));
        {
            let exited = exited.clone();
            let row = row.clone();
            glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                if !exited.get() {
                    row.set_subtitle(&crate::i18n::tr("mounted ✓"));
                }
            });
        }

        // Watch the process: when it exits (after unmount or on failure), clean up.
        let this = self.clone();
        glib::spawn_future_local(async move {
            while let Ok(ev) = events.recv().await {
                match ev {
                    ProcessEvent::Error(e) => row.set_subtitle(&format!("error: {e}")),
                    ProcessEvent::Finished { success, .. } => {
                        exited.set(true);
                        if !success {
                            row.set_subtitle(&crate::i18n::tr(
                                "failed — see a terminal for details",
                            ));
                        }
                        this.remove_mount(id);
                        break;
                    }
                    _ => {}
                }
            }
        });
    }

    fn unmount(&self, id: u64, mountpoint: &str) {
        if let Some(row) = self.row_of(id) {
            row.set_subtitle(&crate::i18n::tr("unmounting…"));
        }
        let rx = capture(UNMOUNT_BIN, unmount_args(mountpoint));
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rx.recv().await {
                // Success: the rclone process exits and its Finished handler removes the row.
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    if let Some(row) = this.row_of(id) {
                        row.set_subtitle(&format!("unmount failed: {e}"));
                    }
                }
                Err(_) => {}
            }
        });
    }

    fn row_of(&self, id: u64) -> Option<adw::ActionRow> {
        self.active
            .borrow()
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.row.clone())
    }

    fn remove_mount(&self, id: u64) {
        let mut v = self.active.borrow_mut();
        if let Some(pos) = v.iter().position(|m| m.id == id) {
            let m = v.remove(pos);
            // Ensure the process is gone even if it exited on its own.
            m.handle.cancel();
            self.active_list.remove(&m.row);
        }
        drop(v);
        self.refresh_active_visibility();
    }

    fn refresh_active_visibility(&self) {
        let empty = self.active.borrow().is_empty();
        self.active_empty.set_visible(empty);
        self.active_list.set_visible(!empty);
    }
}
