//! Add-remote dialog: configure a new rclone remote (Google Drive, Dropbox,
//! S3, SFTP, …) from within the app, then refresh the remotes list.

use std::rc::Rc;

use adw::prelude::*;

use cascade_core::process::{spawn, ProcessEvent};
use cascade_core::rclone::config;

/// Present the dialog. `on_done` is called after a remote is created so the
/// caller can refresh its list of remotes.
pub fn present(parent: &adw::ApplicationWindow, on_done: Rc<dyn Fn()>) {
    let providers = config::providers();

    let name = adw::EntryRow::builder()
        .title(crate::i18n::tr("Remote name (e.g. gdrive)"))
        .build();
    let provider = adw::ComboRow::builder()
        .title(crate::i18n::tr("Provider"))
        .build();
    let labels: Vec<String> = providers.iter().map(|p| crate::i18n::tr(p.label)).collect();
    provider.set_model(Some(&gtk::StringList::new(
        &labels.iter().map(String::as_str).collect::<Vec<_>>(),
    )));
    let params = adw::EntryRow::builder()
        .title(crate::i18n::tr("Parameters (key=value …)"))
        .build();

    let form = adw::PreferencesGroup::builder()
        .title(crate::i18n::tr("New remote"))
        .build();
    form.add(&name);
    form.add(&provider);
    form.add(&params);

    let hint = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .css_classes(vec!["dim-label".to_string()])
        .build();

    // Keep the hint in sync with the chosen provider.
    let update_hint = {
        let provider = provider.clone();
        let hint = hint.clone();
        let providers = providers.clone();
        Rc::new(move || {
            if let Some(p) = providers.get(provider.selected() as usize) {
                hint.set_label(&crate::i18n::tr(p.hint));
            }
        })
    };
    {
        let update_hint = update_hint.clone();
        provider.connect_selected_notify(move |_| update_hint());
    }
    update_hint();

    let create = gtk::Button::builder()
        .label(crate::i18n::tr("Create remote"))
        .halign(gtk::Align::End)
        .css_classes(vec!["pill".to_string(), "suggested-action".to_string()])
        .build();

    let log_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .build();
    let log_buffer = log_view.buffer();
    let scroller = gtk::ScrolledWindow::builder()
        .min_content_height(180)
        .vexpand(true)
        .child(&log_view)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.append(&form);
    content.append(&hint);
    content.append(&create);
    content.append(&scroller);

    let header = adw::HeaderBar::new();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));

    let dialog = adw::Dialog::new();
    dialog.set_title(&crate::i18n::tr("Add remote"));
    dialog.set_content_width(580);
    dialog.set_content_height(540);
    dialog.set_child(Some(&toolbar));

    create.connect_clicked(move |btn| {
        let log = log_buffer.clone();
        log.set_text("");
        let append = move |buf: &gtk::TextBuffer, line: &str| {
            let mut end = buf.end_iter();
            buf.insert(&mut end, line);
            buf.insert(&mut end, "\n");
        };

        let remote_name = name.text().to_string();
        let providers = config::providers();
        let Some(p) = providers.get(provider.selected() as usize).copied() else {
            return;
        };
        let pairs = match config::parse_params(&params.text()) {
            Ok(p) => p,
            Err(e) => {
                append(&log, &format!("✗ {e}"));
                return;
            }
        };
        let argv = match config::config_create_args(&remote_name, p.rtype, &pairs) {
            Ok(a) => a,
            Err(e) => {
                append(&log, &format!("✗ {e}"));
                return;
            }
        };

        append(&log, &format!("Creating “{remote_name}” ({})…", p.rtype));
        if p.oauth {
            append(
                &log,
                "A browser window will open for sign-in. Complete it, then return here.",
            );
        }
        btn.set_sensitive(false);

        let handle = spawn("rclone", argv);
        let events = handle.events.clone();
        let log = log.clone();
        let btn = btn.clone();
        let on_done = on_done.clone();
        glib::spawn_future_local(async move {
            let mut ok = false;
            while let Ok(ev) = events.recv().await {
                match ev {
                    ProcessEvent::Stdout(l) | ProcessEvent::Stderr(l) => append(&log, &l),
                    ProcessEvent::Error(e) => append(&log, &format!("[error] {e}")),
                    ProcessEvent::Finished { success, .. } => {
                        ok = success;
                        break;
                    }
                    _ => {}
                }
            }
            if ok {
                append(&log, "✓ Remote created.");
                on_done();
            } else {
                append(&log, "✗ Could not create the remote (see output above).");
            }
            btn.set_sensitive(true);
        });
    });

    dialog.present(Some(parent));
}
