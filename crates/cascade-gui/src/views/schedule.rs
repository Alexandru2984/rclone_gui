//! Schedule dialog: export a job as a systemd **user** timer + service.
//!
//! No internal daemon — systemd runs the job. The units are written under
//! `~/.config/systemd/user/` and enabled with `systemctl --user`.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;

use cascade_core::job::JobSpec;
use cascade_core::process::capture;
use cascade_core::{rclone, rsync, schedule, Tool};

/// Show the scheduling dialog for `spec`, attached to `parent`.
pub fn present(parent: &adw::ApplicationWindow, spec: JobSpec) {
    let dialog = adw::Dialog::new();
    dialog.set_title(&crate::i18n::tr("Schedule job"));
    dialog.set_content_width(560);

    let when = adw::EntryRow::builder()
        .title(crate::i18n::tr("Run on (systemd OnCalendar)"))
        .text("daily")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(crate::i18n::tr("Schedule"))
        .description(crate::i18n::tr(
            "Exports a systemd user timer that runs this job independently of Cascade.",
        ))
        .build();
    group.add(&when);

    let dry_run = adw::SwitchRow::builder()
        .title(crate::i18n::tr("Dry-run schedule"))
        .subtitle(crate::i18n::tr(
            "Safe default: report recurring changes without writing or deleting files",
        ))
        .active(true)
        .build();
    group.add(&dry_run);

    let hint = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(vec!["dim-label".to_string()])
        .label(crate::i18n::tr(
            "Examples:  hourly · daily · weekly · *-*-* 02:00:00 · Mon *-*-* 09:00",
        ))
        .build();

    let status = gtk::Label::builder().xalign(0.0).wrap(true).build();

    let path_snapshot = spec.validate_paths().unwrap_or_default();
    let consent = gtk::CheckButton::with_label(&crate::i18n::tr(
        "I understand that this recurring job can overwrite or delete data without another prompt",
    ));
    consent.set_visible(false);

    let live_warning = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .css_classes(vec!["error".to_string()])
        .label(crate::i18n::tr(
            "⚠ Live schedules run unattended. Disable dry-run only after reviewing the exact paths and deletion limits.",
        ))
        .build();

    let create = gtk::Button::builder()
        .label(crate::i18n::tr("Create schedule"))
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
    content.append(&live_warning);
    content.append(&consent);
    content.append(&create);
    content.append(&status);

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));

    let refresh_gate: Rc<dyn Fn()> = {
        let dry_run = dry_run.clone();
        let consent = consent.clone();
        let live_warning = live_warning.clone();
        let create = create.clone();
        Rc::new(move || {
            let live = !dry_run.is_active();
            consent.set_visible(live);
            live_warning.set_visible(live);
            create.set_sensitive(!live || consent.is_active());
            if live {
                create.add_css_class("destructive-action");
                create.remove_css_class("suggested-action");
            } else {
                create.remove_css_class("destructive-action");
                create.add_css_class("suggested-action");
            }
        })
    };
    {
        let refresh_gate = refresh_gate.clone();
        dry_run.connect_active_notify(move |_| refresh_gate());
    }
    {
        let refresh_gate = refresh_gate.clone();
        consent.connect_toggled(move |_| refresh_gate());
    }
    refresh_gate();

    create.connect_clicked(move |btn| {
        let live = !dry_run.is_active();
        if live && !consent.is_active() {
            status.set_label(&crate::i18n::tr("✗ Explicit confirmation is required."));
            return;
        }
        let mut requested = spec.clone();
        requested.dry_run = !live;
        let current_warnings = match requested.validate_paths() {
            Ok(warnings) => warnings,
            Err(error) => {
                status.set_label(&format!("✗ {error}"));
                return;
            }
        };
        if current_warnings != path_snapshot {
            status.set_label(&crate::i18n::tr(
                "✗ Path safety changed while this dialog was open. Close it, review the job, and try again.",
            ));
            return;
        }
        let (requested, argv) = match requested.prepare_execution() {
            Ok(prepared) => prepared,
            Err(error) => {
                status.set_label(&format!("✗ {error}"));
                return;
            }
        };
        let bin_path = match tool_path(requested.tool) {
            Some(p) => p,
            None => {
                status.set_label(&crate::i18n::tr("✗ The required tool is not installed."));
                return;
            }
        };
        let on_calendar = when.text().trim().to_string();
        if let Err(error) = schedule::validate_on_calendar(&on_calendar) {
            status.set_label(&format!("✗ {error}"));
            return;
        }

        let dir = match systemd_user_dir() {
            Ok(dir) => dir,
            Err(error) => {
                status.set_label(&format!("✗ {error}"));
                return;
            }
        };

        // If notify-send is available, wire an OnFailure= hook so a failed
        // scheduled run raises a desktop notification (not just a journal entry).
        let notify = match rclone::detect::which("notify-send")
            .and_then(|path| std::fs::canonicalize(path).ok())
        {
            Some(path) => {
                let title = crate::i18n::tr("Scheduled backup failed");
                schedule::build_notify_unit(&path.to_string_lossy(), &title).ok()
            }
            None => None,
        };
        let on_failure = notify
            .as_ref()
            .map(|_| schedule::notify_instance_for(&requested.name));

        let units = match schedule::build_units(
            &requested.name,
            &bin_path,
            &argv,
            &on_calendar,
            on_failure.as_deref(),
        ) {
            Ok(units) => units,
            Err(error) => {
                status.set_label(&format!("✗ {error}"));
                return;
            }
        };

        status.set_label(&crate::i18n::tr("Safely installing and enabling the timer…"));
        btn.set_sensitive(false);

        // Stop an existing timer before replacing either half of the pair.
        let timer_name = units.timer_name.clone();
        let replacing = std::fs::symlink_metadata(dir.join(&timer_name)).is_ok();
        let status = status.clone();
        let btn = btn.clone();
        glib::spawn_future_local(async move {
            if replacing {
                let stop = capture(
                    "systemctl",
                    vec![
                        "--user".into(),
                        "disable".into(),
                        "--now".into(),
                        timer_name.clone(),
                    ],
                );
                if !matches!(stop.recv().await, Ok(Ok(_))) {
                    status.set_label("✗ Could not stop the existing timer; no files were replaced");
                    btn.set_sensitive(true);
                    return;
                }
            }

            let mut files = vec![
                (units.service_name.as_str(), units.service.as_str()),
                (units.timer_name.as_str(), units.timer.as_str()),
            ];
            if let Some(notify) = notify.as_deref() {
                files.push((schedule::NOTIFY_UNIT_FILE, notify));
            }
            if let Err(error) = schedule::write_user_units(&dir, &files) {
                status.set_label(&format!("✗ Could not write unit files safely: {error}"));
                btn.set_sensitive(true);
                return;
            }

            let reload = capture("systemctl", vec!["--user".into(), "daemon-reload".into()]);
            if !matches!(reload.recv().await, Ok(Ok(_))) {
                status.set_label("✗ Units were written, but systemd daemon-reload failed");
                btn.set_sensitive(true);
                return;
            }
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
    info.and_then(|info| std::fs::canonicalize(info.path).ok())
        .map(|path| path.to_string_lossy().into_owned())
}

pub(crate) fn systemd_user_dir() -> std::io::Result<PathBuf> {
    let cfg = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".config"))
        })
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no absolute XDG_CONFIG_HOME or HOME is available",
            )
        })?;
    Ok(cfg.join("systemd/user"))
}
