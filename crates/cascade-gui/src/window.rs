//! Main window shell: a header with a view switcher over an AdwViewStack.
//!
//! Two pages are wired in this skeleton:
//! - **Dashboard** — shows rclone/rsync detection status.
//! - **New Job** — a minimal, *safe* demo: builds an rsync `--dry-run` argv,
//!   runs it through `cascade-core`, and streams the live, sanitized output into
//!   a log view. This is the canonical example of bridging the core's Tokio
//!   process events into the GLib main loop.
//!
//! Phase 2+ adds the remaining screens (Remote Browser, Jobs Queue, Profiles…).

use adw::prelude::*; // re-exports gtk::prelude

use cascade_core::process::{spawn, ProcessEvent};
use cascade_core::rclone::command::preview;
use cascade_core::rsync::{build_args, RsyncOptions};
use cascade_core::security::path;

pub struct MainWindow;

impl MainWindow {
    pub fn new(app: &adw::Application) -> adw::ApplicationWindow {
        let stack = adw::ViewStack::new();
        stack.add_titled_with_icon(
            &dashboard_page(),
            Some("dashboard"),
            "Dashboard",
            "go-home-symbolic",
        );
        stack.add_titled_with_icon(
            &new_job_page(),
            Some("new-job"),
            "New Job",
            "list-add-symbolic",
        );

        let switcher = adw::ViewSwitcher::builder()
            .stack(&stack)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();

        let header = adw::HeaderBar::builder().title_widget(&switcher).build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&stack));

        adw::ApplicationWindow::builder()
            .application(app)
            .title("Cascade")
            .default_width(900)
            .default_height(620)
            .content(&toolbar)
            .build()
    }
}

/// Dashboard: detection status for the external tools.
fn dashboard_page() -> gtk::Widget {
    let group = adw::PreferencesGroup::builder()
        .title("Tools")
        .description("Cascade orchestrates these external programs")
        .build();

    let rclone_row = adw::ActionRow::builder().title("rclone").build();
    match cascade_core::rclone::detect() {
        Some(info) => {
            rclone_row.set_subtitle(&info.version);
            rclone_row.add_suffix(&status_icon(true));
        }
        None => {
            rclone_row.set_subtitle("not installed — install it to enable cloud remotes");
            rclone_row.add_suffix(&status_icon(false));
        }
    }
    group.add(&rclone_row);

    let rsync_row = adw::ActionRow::builder().title("rsync").build();
    match cascade_core::rsync::detect() {
        Some(info) => {
            rsync_row.set_subtitle(&info.version);
            rsync_row.add_suffix(&status_icon(true));
        }
        None => {
            rsync_row.set_subtitle("not installed");
            rsync_row.add_suffix(&status_icon(false));
        }
    }
    group.add(&rsync_row);

    let page = adw::PreferencesPage::new();
    page.add(&group);
    page.upcast()
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

/// New Job: a safe rsync dry-run demo with live, sanitized output.
fn new_job_page() -> gtk::Widget {
    // Prefill with throwaway temp dirs so the demo runs with zero risk.
    let src = std::env::temp_dir().join("cascade_demo_src");
    let dst = std::env::temp_dir().join("cascade_demo_dst");
    let _ = std::fs::create_dir_all(&src);
    let _ = std::fs::create_dir_all(&dst);
    let _ = std::fs::write(src.join("example.txt"), b"demo");

    let source_row = adw::EntryRow::builder()
        .title("Source")
        .text(format!("{}/", src.display()))
        .build();
    let dest_row = adw::EntryRow::builder()
        .title("Destination")
        .text(format!("{}/", dst.display()))
        .build();

    let io_group = adw::PreferencesGroup::builder().title("Paths").build();
    io_group.add(&source_row);
    io_group.add(&dest_row);

    // Command preview (monospace, display-only).
    let preview_label = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(vec!["monospace".to_string(), "dim-label".to_string()])
        .label("(press “Dry-run” to generate the command)")
        .build();
    let preview_group = adw::PreferencesGroup::builder()
        .title("Command preview")
        .build();
    preview_group.add(&preview_label);

    // Log view.
    let log_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .min_content_height(220)
        .vexpand(true)
        .child(&log_view)
        .build();
    let log_group = adw::PreferencesGroup::builder()
        .title("Live output")
        .build();
    log_group.add(&scroller);

    // Action button — clearly a dry-run, the safe default.
    let run_btn = gtk::Button::builder()
        .label("Dry-run")
        .css_classes(vec!["suggested-action".to_string()])
        .build();
    let btn_box = gtk::Box::builder().halign(gtk::Align::End).build();
    btn_box.append(&run_btn);

    run_btn.connect_clicked(glib::clone!(
        #[weak]
        source_row,
        #[weak]
        dest_row,
        #[weak]
        preview_label,
        #[weak]
        log_view,
        move |btn| {
            let source = source_row.text().to_string();
            let dest = dest_row.text().to_string();
            let buffer = log_view.buffer();
            buffer.set_text("");

            // Validate before doing anything.
            for (label, p) in [("source", &source), ("destination", &dest)] {
                if let Err(e) = path::validate(p) {
                    append(&buffer, &format!("✗ {label}: {e}\n"));
                    return;
                }
            }

            let opts = RsyncOptions {
                dry_run: true,
                ..Default::default()
            };
            let args = match build_args(&source, &dest, &opts) {
                Ok(a) => a,
                Err(e) => {
                    append(&buffer, &format!("✗ {e}\n"));
                    return;
                }
            };
            preview_label.set_label(&preview("rsync", &args));

            btn.set_sensitive(false);
            let handle = spawn("rsync", args);
            let events = handle.events.clone();

            // Bridge Tokio process events → GLib main loop. UI never blocks.
            glib::spawn_future_local(glib::clone!(
                #[weak]
                buffer,
                #[weak]
                btn,
                async move {
                    while let Ok(ev) = events.recv().await {
                        match ev {
                            ProcessEvent::Started { pid } => {
                                append(&buffer, &format!("[started pid={pid:?}]\n"))
                            }
                            ProcessEvent::Stdout(line) => append(&buffer, &format!("{line}\n")),
                            ProcessEvent::Stderr(line) => append(&buffer, &format!("! {line}\n")),
                            ProcessEvent::Error(e) => append(&buffer, &format!("[error] {e}\n")),
                            ProcessEvent::Finished { success, code } => {
                                append(
                                    &buffer,
                                    &format!("[finished success={success} code={code:?}]\n"),
                                );
                                break;
                            }
                        }
                    }
                    btn.set_sensitive(true);
                }
            ));
        }
    ));

    let page = adw::PreferencesPage::new();
    page.add(&io_group);
    page.add(&preview_group);
    page.add(&log_group);

    // Wrap so we can append the button row beneath the preferences page.
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);
    outer.append(&page);
    outer.append(&btn_box);
    outer.upcast()
}

fn append(buffer: &gtk::TextBuffer, text: &str) {
    let mut end = buffer.end_iter();
    buffer.insert(&mut end, text);
}
