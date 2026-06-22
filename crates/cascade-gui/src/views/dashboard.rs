//! Dashboard: tool detection + a controllable local rclone RC daemon.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;

use cascade_core::process::capture_env;
use cascade_core::rclone::rcd::{parse_version, Rcd};

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
