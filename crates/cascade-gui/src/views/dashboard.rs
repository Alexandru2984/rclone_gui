//! Dashboard: tool detection + a controllable local rclone RC daemon.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;

use cascade_core::process::{capture, capture_env};
use cascade_core::rclone::rcd::{parse_version, Rcd};

/// A reusable "rebuild the schedules list" callback.
type Refresh = Rc<dyn Fn()>;

pub fn build() -> gtk::Widget {
    let tools = adw::PreferencesGroup::builder()
        .title("Tools")
        .description("Cascade orchestrates these external programs")
        .build();
    tools.add(&tool_row(
        "rclone",
        cascade_core::rclone::detect().map(|i| i.version),
        "not installed — install it to enable cloud remotes",
    ));
    tools.add(&tool_row(
        "rsync",
        cascade_core::rsync::detect().map(|i| i.version),
        "not installed",
    ));

    // Local RC daemon control.
    let status = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .label("Not running.")
        .css_classes(vec!["dim-label".to_string()])
        .build();
    let toggle = gtk::Button::builder()
        .label("Start local RC daemon")
        .halign(gtk::Align::Start)
        .css_classes(vec!["pill".to_string()])
        .build();

    let row_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    row_box.append(&status);
    row_box.append(&toggle);
    let rcd_group = adw::PreferencesGroup::builder()
        .title("rclone RC daemon")
        .description("A local control daemon bound to 127.0.0.1 with random credentials — never exposed to the network")
        .build();
    rcd_group.add(&row_box);

    let state: Rc<RefCell<Option<Rcd>>> = Rc::new(RefCell::new(None));
    {
        let state = state.clone();
        let status = status.clone();
        toggle.connect_clicked(move |btn| {
            let mut st = state.borrow_mut();
            if let Some(rcd) = st.take() {
                rcd.stop();
                status.set_label("Not running.");
                btn.set_label("Start local RC daemon");
                return;
            }
            match Rcd::start() {
                Ok(rcd) => {
                    let addr = rcd.addr().to_string();
                    let args = rcd.rc_args("core/version");
                    let env = rcd.rc_env();
                    status.set_label(&format!("Starting on {addr} …"));
                    btn.set_label("Stop");
                    *st = Some(rcd);
                    drop(st);

                    // Give the daemon a moment to bind, then confirm via RC.
                    let status = status.clone();
                    glib::timeout_add_local_once(Duration::from_millis(800), move || {
                        let rx = capture_env("rclone", args, env);
                        glib::spawn_future_local(async move {
                            match rx.recv().await {
                                Ok(Ok(out)) => {
                                    let v = parse_version(&out).unwrap_or_else(|| "?".into());
                                    status.set_label(&format!("Running on {addr} · rclone {v}"));
                                }
                                Ok(Err(e)) => {
                                    status.set_label(&format!("Started, but RC check failed: {e}"))
                                }
                                Err(_) => {}
                            }
                        });
                    });
                }
                Err(e) => status.set_label(&format!("Could not start: {e}")),
            }
        });
    }

    let page = adw::PreferencesPage::new();
    page.add(&tools);
    page.add(&rcd_group);
    page.add(&schedules_group());
    page.upcast()
}

/// A group listing the systemd user timers Cascade created, each removable.
fn schedules_group() -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder()
        .title("Scheduled jobs")
        .description("systemd user timers created via “Schedule…”")
        .build();
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list".to_string()])
        .build();
    let empty = gtk::Label::builder()
        .label("No schedules yet.")
        .xalign(0.0)
        .css_classes(vec!["dim-label".to_string()])
        .build();
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.append(&empty);
    column.append(&list);
    group.add(&column);

    // Self-referential refresh: delete handlers call back into it.
    let refresh_cell: Rc<RefCell<Option<Refresh>>> = Rc::new(RefCell::new(None));
    let refresh: Refresh = {
        let list = list.clone();
        let empty = empty.clone();
        let refresh_cell = refresh_cell.clone();
        Rc::new(move || {
            list.remove_all();
            let timers = list_cascade_timers();
            empty.set_visible(timers.is_empty());
            list.set_visible(!timers.is_empty());
            for (file, on_calendar) in timers {
                let title = file
                    .trim_start_matches("cascade-")
                    .trim_end_matches(".timer");
                let row = adw::ActionRow::builder()
                    .title(title)
                    .subtitle(&on_calendar)
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name("alarm-symbolic"));
                let del = gtk::Button::builder()
                    .icon_name("user-trash-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(vec!["flat".to_string()])
                    .tooltip_text("Remove this schedule")
                    .build();
                let refresh_cell = refresh_cell.clone();
                let file = file.clone();
                del.connect_clicked(move |_| {
                    let again = refresh_cell.borrow().clone();
                    delete_timer(file.clone(), again);
                });
                row.add_suffix(&del);
                list.append(&row);
            }
        })
    };
    *refresh_cell.borrow_mut() = Some(refresh.clone());
    refresh();

    group
}

/// List Cascade-created timers as `(filename, OnCalendar)`.
fn list_cascade_timers() -> Vec<(String, String)> {
    let dir = crate::views::schedule::systemd_user_dir();
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("cascade-") && name.ends_with(".timer") {
                let on_cal = std::fs::read_to_string(entry.path())
                    .ok()
                    .and_then(|c| cascade_core::schedule::parse_on_calendar(&c))
                    .unwrap_or_else(|| "?".into());
                out.push((name, on_cal));
            }
        }
    }
    out.sort();
    out
}

/// Disable + remove a timer (and its service), then refresh the list.
fn delete_timer(timer_file: String, refresh: Option<Refresh>) {
    let rx = capture(
        "systemctl",
        vec![
            "--user".into(),
            "disable".into(),
            "--now".into(),
            timer_file.clone(),
        ],
    );
    glib::spawn_future_local(async move {
        let _ = rx.recv().await;
        let dir = crate::views::schedule::systemd_user_dir();
        let _ = std::fs::remove_file(format!("{dir}/{timer_file}"));
        let _ = std::fs::remove_file(format!(
            "{dir}/{}",
            timer_file.replace(".timer", ".service")
        ));
        let reload = capture("systemctl", vec!["--user".into(), "daemon-reload".into()]);
        let _ = reload.recv().await;
        if let Some(refresh) = refresh {
            refresh();
        }
    });
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
