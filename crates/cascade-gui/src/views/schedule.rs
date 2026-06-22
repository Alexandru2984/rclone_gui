//! Schedule dialog: export a job as a systemd **user** timer + service.
//!
//! No internal daemon — systemd runs the job. The units are written under
//! `~/.config/systemd/user/` and enabled with `systemctl --user`.

use adw::prelude::*;

use cascade_core::job::JobSpec;
use cascade_core::process::capture;
use cascade_core::{rclone, rsync, schedule, Tool};

/// Show the scheduling dialog for `spec`, attached to `parent`.
pub fn present(parent: &adw::ApplicationWindow, spec: JobSpec) {
    let dialog = adw::Dialog::new();
    dialog.set_title("Schedule job");
    dialog.set_content_width(560);

    let when = adw::EntryRow::builder()
        .title("Run on (systemd OnCalendar)")
        .text("daily")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("Schedule")
        .description("Exports a systemd user timer that runs this job independently of Cascade.")
        .build();
    group.add(&when);

    let hint = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(vec!["dim-label".to_string()])
        .label("Examples:  hourly · daily · weekly · *-*-* 02:00:00 · Mon *-*-* 09:00")
        .build();

    let status = gtk::Label::builder().xalign(0.0).wrap(true).build();

    let create = gtk::Button::builder()
        .label("Create schedule")
        .halign(gtk::Align::End)
        .css_classes(vec!["pill".to_string(), "suggested-action".to_string()])
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.append(&group);
    content.append(&hint);
    content.append(&create);
    content.append(&status);

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));

    create.connect_clicked(move |btn| {
        // Never write a credential into a (world-readable) unit file.
        if spec.contains_secret() {
            status.set_label(
                "✗ This job embeds a credential. Configure an rclone remote and reference it instead.",
            );
            return;
        }
        let bin_path = match tool_path(spec.tool) {
            Some(p) => p,
            None => {
                status.set_label("✗ The required tool is not installed.");
                return;
            }
        };
        let argv = match spec.build_argv() {
            Ok(a) => a,
            Err(e) => {
                status.set_label(&format!("✗ {e}"));
                return;
            }
        };
        let on_calendar = when.text().trim().to_string();
        if on_calendar.is_empty() {
            status.set_label("✗ Enter a schedule (e.g. daily).");
            return;
        }

        let units = schedule::build_units(&spec.name, &bin_path, &argv, &on_calendar);
        let dir = systemd_user_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            status.set_label(&format!("✗ Could not create {dir}: {e}"));
            return;
        }
        let service_path = format!("{dir}/{}", units.service_name);
        let timer_path = format!("{dir}/{}", units.timer_name);
        if let Err(e) = std::fs::write(&service_path, &units.service)
            .and_then(|_| std::fs::write(&timer_path, &units.timer))
        {
            status.set_label(&format!("✗ Could not write unit files: {e}"));
            return;
        }

        status.set_label("Enabling the timer…");
        btn.set_sensitive(false);

        // daemon-reload, then enable --now the timer.
        let timer_name = units.timer_name.clone();
        let status = status.clone();
        let btn = btn.clone();
        glib::spawn_future_local(async move {
            let reload = capture("systemctl", vec!["--user".into(), "daemon-reload".into()]);
            let _ = reload.recv().await;
            let rx = capture(
                "systemctl",
                vec!["--user".into(), "enable".into(), "--now".into(), timer_name.clone()],
            );
            match rx.recv().await {
                Ok(Ok(_)) => status.set_label(&format!(
                    "✓ Scheduled. Manage with: systemctl --user list-timers · status {timer_name}"
                )),
                Ok(Err(e)) => status.set_label(&format!("✗ systemctl: {e}")),
                Err(_) => {}
            }
            btn.set_sensitive(true);
        });
    });

    dialog.present(Some(parent));
}

fn tool_path(tool: Tool) -> Option<String> {
    let info = match tool {
        Tool::Rclone => rclone::detect(),
        Tool::Rsync => rsync::detect(),
    };
    info.map(|i| i.path.to_string_lossy().into_owned())
}

fn systemd_user_dir() -> String {
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    format!("{cfg}/systemd/user")
}
