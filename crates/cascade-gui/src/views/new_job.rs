//! New Job: pick tool + operation + paths, preview the exact command, gate
//! destructive runs behind a confirmation, run it live, and persist to SQLite.
//!
//! This is the heart of Phase 1: a full, safe, end-to-end flow for both tools.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::gio;

use cascade_core::job::{AdvancedOptions, JobSpec, OpKind};
use cascade_core::logs::LogWriter;
use cascade_core::process::{progress, spawn_with_parser, LineParser, ProcessEvent, RunHandle};
use cascade_core::security::destructive::RiskLevel;
use cascade_core::security::{flags, path};
use cascade_core::Tool;

use crate::ctx::AppCtx;

/// Owns the screen's widgets and behavior. Shared as `Rc<Inputs>` so signal
/// handlers can call back into it.
struct Inputs {
    ctx: Rc<AppCtx>,
    window: adw::ApplicationWindow,

    name: adw::EntryRow,
    tool: adw::ComboRow,
    op: adw::ComboRow,
    source: adw::EntryRow,
    dest: adw::EntryRow,
    delete: adw::SwitchRow,

    // Advanced options
    adv_excludes: adw::EntryRow,
    adv_includes: adw::EntryRow,
    adv_transfers: adw::SpinRow,
    adv_checkers: adw::SpinRow,
    adv_bwlimit: adw::EntryRow,
    adv_retries: adw::SpinRow,
    adv_checksum: adw::SwitchRow,
    adv_compress: adw::SwitchRow,
    adv_ssh_port: adw::SpinRow,
    adv_custom: adw::EntryRow,

    preview: gtk::Label,
    risk: gtk::Label,

    progress_bar: gtk::ProgressBar,
    progress_label: gtk::Label,

    log_view: gtk::TextView,
    log_buffer: gtk::TextBuffer,

    run_btn: gtk::Button,
    dry_btn: gtk::Button,
    cancel_btn: gtk::Button,
    save_btn: gtk::Button,

    /// The currently running child, if any — kept so Cancel can reach it.
    current: RefCell<Option<RunHandle>>,

    /// Callback to refresh sibling screens (History, Profiles) after a change.
    on_changed: Rc<dyn Fn()>,
}

/// Public handle to the New Job screen: its root widget plus the ability to
/// load a spec into it (used by the Profiles screen).
pub struct NewJobView {
    root: gtk::Widget,
    inputs: Rc<Inputs>,
}

impl NewJobView {
    pub fn widget(&self) -> &gtk::Widget {
        &self.root
    }

    /// Populate the form from a saved profile's spec.
    pub fn load_spec(&self, spec: &JobSpec) {
        self.inputs.apply_spec(spec);
    }

    /// Set the source path (used by the Remote/Local browser pickers).
    pub fn set_source(&self, path: &str) {
        self.inputs.source.set_text(path);
        self.inputs.refresh_preview();
    }

    /// Set the destination path (used by the Remote/Local browser pickers).
    pub fn set_destination(&self, path: &str) {
        self.inputs.dest.set_text(path);
        self.inputs.refresh_preview();
    }
}

/// Build the New Job screen. `on_changed` is invoked after a run completes so
/// the caller can refresh the History view.
pub fn build(
    ctx: Rc<AppCtx>,
    window: adw::ApplicationWindow,
    on_changed: Rc<dyn Fn()>,
    on_enqueue: Rc<dyn Fn(JobSpec)>,
) -> NewJobView {
    // Prefill with throwaway temp dirs so the screen is immediately runnable.
    let src = std::env::temp_dir().join("cascade_demo_src");
    let dst = std::env::temp_dir().join("cascade_demo_dst");
    let _ = std::fs::create_dir_all(&src);
    let _ = std::fs::create_dir_all(&dst);
    let _ = std::fs::write(src.join("example.txt"), b"demo");

    let name = adw::EntryRow::builder().title("Name (optional)").build();

    let tool = adw::ComboRow::builder().title("Tool").build();
    tool.set_model(Some(&gtk::StringList::new(&["rsync", "rclone"])));
    let op = adw::ComboRow::builder().title("Operation").build();
    op.set_model(Some(&gtk::StringList::new(&[
        "Copy",
        "Sync (mirror)",
        "Move",
    ])));

    let job_group = adw::PreferencesGroup::builder().title("Job").build();
    job_group.add(&name);
    job_group.add(&tool);
    job_group.add(&op);

    let source = adw::EntryRow::builder()
        .title("Source")
        .text(format!("{}/", src.display()))
        .build();
    let dest = adw::EntryRow::builder()
        .title("Destination")
        .text(format!("{}/", dst.display()))
        .build();
    // Folder-picker buttons are added and wired in `connect_browse` during `wire`.

    let paths_group = adw::PreferencesGroup::builder().title("Paths").build();
    paths_group.add(&source);
    paths_group.add(&dest);

    let delete = adw::SwitchRow::builder()
        .title("Delete extra files at destination")
        .subtitle("Applies to Copy. Sync and Move always remove on their own.")
        .build();
    let opts_group = adw::PreferencesGroup::builder().title("Options").build();
    opts_group.add(&delete);

    // Advanced options, collapsed by default.
    let adv_excludes = adw::EntryRow::builder()
        .title("Exclude patterns (comma-separated)")
        .build();
    let adv_includes = adw::EntryRow::builder()
        .title("Include patterns (comma-separated)")
        .build();
    let adv_transfers = spin_row("Parallel transfers — rclone (0 = default)", 0.0, 64.0);
    let adv_checkers = spin_row("Checkers — rclone (0 = default)", 0.0, 64.0);
    let adv_bwlimit = adw::EntryRow::builder()
        .title("Bandwidth limit — rclone (e.g. 10M)")
        .build();
    let adv_retries = spin_row("Retries — rclone (0 = default)", 0.0, 20.0);
    let adv_checksum = adw::SwitchRow::builder()
        .title("Verify with checksum")
        .build();
    let adv_compress = adw::SwitchRow::builder()
        .title("Compress in transit — rsync (-z)")
        .build();
    let adv_ssh_port = spin_row("SSH port — rsync (0 = default)", 0.0, 65535.0);
    let adv_custom = adw::EntryRow::builder()
        .title("Custom flags (quoted, space-separated)")
        .build();

    let advanced = adw::ExpanderRow::builder()
        .title("Advanced options")
        .subtitle("Patterns, performance, and custom flags")
        .build();
    for row in [&adv_excludes, &adv_includes, &adv_bwlimit, &adv_custom] {
        advanced.add_row(row);
    }
    advanced.add_row(&adv_transfers);
    advanced.add_row(&adv_checkers);
    advanced.add_row(&adv_retries);
    advanced.add_row(&adv_ssh_port);
    advanced.add_row(&adv_checksum);
    advanced.add_row(&adv_compress);
    let adv_group = adw::PreferencesGroup::new();
    adv_group.add(&advanced);

    let preview = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(vec!["monospace".to_string()])
        .label("…")
        .build();
    let risk = gtk::Label::builder().xalign(0.0).build();
    let cmd_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    cmd_box.append(&preview);
    cmd_box.append(&risk);
    let cmd_group = adw::PreferencesGroup::builder()
        .title("Command preview")
        .build();
    cmd_group.add(&cmd_box);

    let progress_bar = gtk::ProgressBar::builder()
        .show_text(false)
        .visible(false)
        .build();
    let progress_label = gtk::Label::builder()
        .xalign(0.0)
        .visible(false)
        .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
        .build();
    let progress_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    progress_box.append(&progress_bar);
    progress_box.append(&progress_label);
    let progress_group = adw::PreferencesGroup::builder().title("Progress").build();
    progress_group.add(&progress_box);

    let log_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .build();
    let log_buffer = log_view.buffer();
    let scroller = gtk::ScrolledWindow::builder()
        .min_content_height(200)
        .vexpand(true)
        .child(&log_view)
        .build();
    let log_group = adw::PreferencesGroup::builder()
        .title("Live output")
        .build();
    log_group.add(&scroller);

    let save_btn = gtk::Button::builder()
        .label(crate::i18n::tr("Save profile"))
        .css_classes(vec!["pill".to_string()])
        .build();
    let queue_btn = gtk::Button::builder()
        .label(crate::i18n::tr("Add to queue"))
        .css_classes(vec!["pill".to_string()])
        .build();
    let schedule_btn = gtk::Button::builder()
        .label(crate::i18n::tr("Schedule…"))
        .css_classes(vec!["pill".to_string()])
        .build();
    let cancel_btn = gtk::Button::builder()
        .label(crate::i18n::tr("Cancel"))
        .css_classes(vec!["pill".to_string(), "destructive-action".to_string()])
        .sensitive(false)
        .build();
    let dry_btn = gtk::Button::builder()
        .label(crate::i18n::tr("Dry-run"))
        .css_classes(vec!["pill".to_string()])
        .build();
    let run_btn = gtk::Button::builder()
        .label(crate::i18n::tr("Start"))
        .css_classes(vec!["pill".to_string(), "suggested-action".to_string()])
        .build();
    let btn_box = gtk::Box::builder()
        .halign(gtk::Align::End)
        .spacing(8)
        .build();
    btn_box.append(&save_btn);
    btn_box.append(&queue_btn);
    btn_box.append(&schedule_btn);
    btn_box.append(&cancel_btn);
    btn_box.append(&dry_btn);
    btn_box.append(&run_btn);

    let page = adw::PreferencesPage::new();
    page.add(&job_group);
    page.add(&paths_group);
    page.add(&opts_group);
    page.add(&adv_group);
    page.add(&cmd_group);
    page.add(&progress_group);
    page.add(&log_group);

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);
    outer.append(&page);
    outer.append(&btn_box);

    let inputs = Rc::new(Inputs {
        ctx,
        window,
        name,
        tool,
        op,
        source,
        dest,
        delete,
        adv_excludes,
        adv_includes,
        adv_transfers,
        adv_checkers,
        adv_bwlimit,
        adv_retries,
        adv_checksum,
        adv_compress,
        adv_ssh_port,
        adv_custom,
        preview,
        risk,
        progress_bar,
        progress_label,
        log_view,
        log_buffer,
        run_btn,
        dry_btn,
        cancel_btn,
        save_btn,
        current: RefCell::new(None),
        on_changed,
    });

    inputs.wire();
    inputs.refresh_preview();

    // "Add to queue" reads the current form and hands the spec to the queue.
    {
        let inputs = inputs.clone();
        queue_btn.connect_clicked(move |_| match inputs.read_spec() {
            Ok(spec) => {
                on_enqueue(spec);
                inputs.log_line("✓ added to queue");
            }
            Err(e) => inputs.log_line(&format!("✗ {e}")),
        });
    }

    // "Schedule…" exports the current job as a systemd user timer.
    {
        let inputs = inputs.clone();
        schedule_btn.connect_clicked(move |_| match inputs.read_spec() {
            Ok(spec) => crate::views::schedule::present(&inputs.window, spec),
            Err(e) => inputs.log_line(&format!("✗ {e}")),
        });
    }

    NewJobView {
        root: outer.upcast(),
        inputs,
    }
}

fn spin_row(title: &str, lower: f64, upper: f64) -> adw::SpinRow {
    adw::SpinRow::builder()
        .title(title)
        .adjustment(&gtk::Adjustment::new(0.0, lower, upper, 1.0, 1.0, 0.0))
        .build()
}

/// A spin value of 0 means "unset" → `None`.
fn spin_opt(row: &adw::SpinRow) -> Option<u32> {
    let v = row.value() as u32;
    if v == 0 {
        None
    } else {
        Some(v)
    }
}

/// Split a comma-separated patterns field into trimmed, non-empty items.
fn split_csv(text: &str) -> Vec<String> {
    text.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn browse_button() -> gtk::Button {
    let b = gtk::Button::from_icon_name("folder-open-symbolic");
    b.add_css_class("flat");
    b.set_valign(gtk::Align::Center);
    b.set_tooltip_text(Some("Choose a folder"));
    b
}

impl Inputs {
    /// Connect all signals that affect the preview, plus the action buttons.
    fn wire(self: &Rc<Self>) {
        macro_rules! on {
            ($widget:expr, $connect:ident) => {{
                let this = self.clone();
                $widget.$connect(move |_| this.refresh_preview());
            }};
        }
        on!(self.tool, connect_selected_notify);
        on!(self.op, connect_selected_notify);
        on!(self.source, connect_changed);
        on!(self.dest, connect_changed);
        on!(self.delete, connect_active_notify);
        on!(self.adv_excludes, connect_changed);
        on!(self.adv_includes, connect_changed);
        on!(self.adv_bwlimit, connect_changed);
        on!(self.adv_custom, connect_changed);
        on!(self.adv_transfers, connect_value_notify);
        on!(self.adv_checkers, connect_value_notify);
        on!(self.adv_retries, connect_value_notify);
        on!(self.adv_ssh_port, connect_value_notify);
        on!(self.adv_checksum, connect_active_notify);
        on!(self.adv_compress, connect_active_notify);

        // Add + wire a folder-picker button onto each path row.
        connect_browse(self, &self.source);
        connect_browse(self, &self.dest);

        {
            let this = self.clone();
            self.dry_btn.connect_clicked(move |_| this.run(true));
        }
        {
            let this = self.clone();
            self.run_btn.connect_clicked(move |_| this.on_start());
        }
        {
            let this = self.clone();
            self.cancel_btn.connect_clicked(move |_| this.cancel());
        }
        {
            let this = self.clone();
            self.save_btn.connect_clicked(move |_| this.save_profile());
        }
    }

    /// Save the current form as a named profile.
    fn save_profile(&self) {
        match self.read_spec() {
            Ok(spec) => match self.ctx.store.save_profile(&spec) {
                Ok(_) => {
                    self.log_line(&format!("✓ saved profile “{}”", spec.name));
                    (self.on_changed)();
                }
                Err(e) => self.log_line(&format!("✗ could not save profile: {e}")),
            },
            Err(e) => self.log_line(&format!("✗ {e}")),
        }
    }

    /// Load a spec into the form widgets.
    fn apply_spec(&self, spec: &JobSpec) {
        self.name.set_text(&spec.name);
        self.tool.set_selected(match spec.tool {
            Tool::Rclone => 1,
            Tool::Rsync => 0,
        });
        self.op.set_selected(match spec.op {
            OpKind::Copy => 0,
            OpKind::Sync => 1,
            OpKind::Move => 2,
        });
        self.source.set_text(&spec.source);
        self.dest.set_text(&spec.destination);
        self.delete.set_active(spec.delete);

        let o = &spec.options;
        self.adv_excludes.set_text(&o.excludes.join(", "));
        self.adv_includes.set_text(&o.includes.join(", "));
        self.adv_transfers
            .set_value(o.transfers.unwrap_or(0) as f64);
        self.adv_checkers.set_value(o.checkers.unwrap_or(0) as f64);
        self.adv_bwlimit
            .set_text(o.bwlimit.as_deref().unwrap_or(""));
        self.adv_retries.set_value(o.retries.unwrap_or(0) as f64);
        self.adv_checksum.set_active(o.checksum);
        self.adv_compress.set_active(o.compress);
        self.adv_ssh_port.set_value(o.ssh_port.unwrap_or(0) as f64);
        self.adv_custom.set_text(&o.extra_flags.join(" "));

        self.refresh_preview();
    }

    fn cancel(&self) {
        if let Some(handle) = self.current.borrow().as_ref() {
            handle.cancel();
            self.log_line("[cancelling…]");
            self.cancel_btn.set_sensitive(false);
        }
    }

    /// Read the current widget state into a `JobSpec`, or a user-facing error.
    fn read_spec(&self) -> Result<JobSpec, String> {
        let source = self.source.text().to_string();
        let dest = self.dest.text().to_string();
        if source.trim().is_empty() {
            return Err("Source is empty".into());
        }
        if dest.trim().is_empty() {
            return Err("Destination is empty".into());
        }
        for (label, p) in [("Source", &source), ("Destination", &dest)] {
            if !path::is_remote_endpoint(p) {
                if let Err(e) = path::validate(p) {
                    return Err(format!("{label}: {e}"));
                }
            }
        }

        let tool = if self.tool.selected() == 1 {
            Tool::Rclone
        } else {
            Tool::Rsync
        };
        let op = match self.op.selected() {
            1 => OpKind::Sync,
            2 => OpKind::Move,
            _ => OpKind::Copy,
        };
        let name = {
            let n = self.name.text().to_string();
            if n.trim().is_empty() {
                format!(
                    "{} {} → {}",
                    op.label(),
                    last_component(&source),
                    last_component(&dest)
                )
            } else {
                n
            }
        };

        let extra_flags =
            flags::parse(&self.adv_custom.text()).map_err(|e| format!("Custom flags: {e}"))?;
        let options = AdvancedOptions {
            excludes: split_csv(&self.adv_excludes.text()),
            includes: split_csv(&self.adv_includes.text()),
            transfers: spin_opt(&self.adv_transfers),
            checkers: spin_opt(&self.adv_checkers),
            bwlimit: {
                let b = self.adv_bwlimit.text().trim().to_string();
                if b.is_empty() {
                    None
                } else {
                    Some(b)
                }
            },
            retries: spin_opt(&self.adv_retries),
            checksum: self.adv_checksum.is_active(),
            compress: self.adv_compress.is_active(),
            ssh_port: spin_opt(&self.adv_ssh_port).map(|v| v as u16),
            extra_flags,
        };

        Ok(JobSpec {
            name,
            tool,
            op,
            source,
            destination: dest,
            dry_run: false,
            delete: self.delete.is_active(),
            options,
        })
    }

    /// Recompute the command preview and risk badge.
    fn refresh_preview(&self) {
        match self.read_spec() {
            Ok(spec) => {
                match spec.preview() {
                    Ok(p) => self.preview.set_label(&p),
                    Err(e) => self.preview.set_label(&format!("error: {e}")),
                }
                self.set_risk(spec.risk());
            }
            Err(msg) => {
                self.preview.set_label(&format!("⚠ {msg}"));
                self.risk.set_label("");
                for c in ["success", "warning", "error"] {
                    self.risk.remove_css_class(c);
                }
            }
        }
    }

    fn set_risk(&self, risk: RiskLevel) {
        let (text, css) = match risk {
            RiskLevel::Safe => ("✓ Safe — nothing is deleted", "success"),
            RiskLevel::Caution => ("• Files may be overwritten at the destination", "warning"),
            RiskLevel::Destructive => (
                "⚠ Destructive — files at the destination may be deleted",
                "error",
            ),
        };
        self.risk.set_label(text);
        for c in ["success", "warning", "error"] {
            self.risk.remove_css_class(c);
        }
        self.risk.add_css_class(css);

        self.run_btn.remove_css_class("destructive-action");
        self.run_btn.remove_css_class("suggested-action");
        if risk == RiskLevel::Destructive {
            self.run_btn.add_css_class("destructive-action");
        } else {
            self.run_btn.add_css_class("suggested-action");
        }
    }

    /// Start handler: confirm first if the operation is destructive.
    fn on_start(self: &Rc<Self>) {
        let spec = match self.read_spec() {
            Ok(s) => s,
            Err(e) => {
                self.log_line(&format!("✗ {e}"));
                return;
            }
        };
        let confirm = self.ctx.settings.borrow().confirm_destructive;
        if spec.risk().requires_confirmation() && confirm {
            self.confirm_destructive(&spec.preview().unwrap_or_default());
        } else {
            self.run(false);
        }
    }

    fn confirm_destructive(self: &Rc<Self>, command_desc: &str) {
        let body = format!(
            "This operation can delete files at the destination.\n\n{command_desc}\n\n\
             Running a dry-run first lets you preview exactly what would change."
        );
        let dialog = adw::AlertDialog::new(Some("Destructive operation"), Some(&body));
        dialog.add_responses(&[
            ("cancel", "Cancel"),
            ("dry", "Dry-run first"),
            ("run", "Run anyway"),
        ]);
        dialog.set_response_appearance("run", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("dry"));
        dialog.set_close_response("cancel");

        let this = self.clone();
        dialog.choose(
            &self.window,
            gio::Cancellable::NONE,
            move |resp| match resp.as_str() {
                "run" => this.run(false),
                "dry" => this.run(true),
                _ => {}
            },
        );
    }

    /// Build the argv, persist a job + run, and stream the process live.
    fn run(self: &Rc<Self>, force_dry: bool) {
        let mut spec = match self.read_spec() {
            Ok(s) => s,
            Err(e) => {
                self.log_line(&format!("✗ {e}"));
                return;
            }
        };
        if force_dry {
            spec.dry_run = true;
        }
        let argv = match spec.build_argv() {
            Ok(a) => a,
            Err(e) => {
                self.log_line(&format!("✗ {e}"));
                return;
            }
        };
        // Sanitized: this value is persisted (DB), written to the log file, and
        // shown later in History/Job Details, so it must not carry secrets.
        let preview = spec.preview_sanitized().unwrap_or_default();

        // Persist the job and the run we are about to start.
        let options_json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".into());
        let job_id = match self.ctx.store.insert_job(
            &spec.name,
            kind_str(spec.tool),
            op_str(spec.op),
            &spec.source,
            &spec.destination,
            &options_json,
        ) {
            Ok(id) => id,
            Err(e) => {
                self.log_line(&format!("✗ database: {e}"));
                return;
            }
        };
        let run_id = self
            .ctx
            .store
            .start_run(job_id, spec.dry_run, &preview)
            .unwrap_or(-1);

        self.clear_log();
        self.log_line(&format!("$ {preview}"));
        self.set_running(true);

        // Pick the right progress parser for the tool.
        let parser: LineParser = match spec.tool {
            Tool::Rsync => Arc::new(progress::parse_rsync),
            Tool::Rclone => Arc::new(progress::parse_rclone),
        };
        let handle = spawn_with_parser(spec.binary(), argv, Some(parser));
        let events = handle.events.clone();
        *self.current.borrow_mut() = Some(handle);

        let job_name = spec.name.clone();
        let preview_log = preview.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            // Per-run on-disk log (sanitized lines only).
            let mut log = LogWriter::create(&this.ctx.paths.log_dir, run_id).ok();
            if let Some(w) = log.as_mut() {
                let _ = w.write_line(&format!("$ {preview_log}"));
            }

            let mut exit_code = None;
            let mut failed = false;
            let mut error_summary: Option<String> = None;
            while let Ok(ev) = events.recv().await {
                match ev {
                    ProcessEvent::Started { pid } => {
                        this.log_line(&format!("[started pid={pid:?}]"))
                    }
                    ProcessEvent::Progress(p) => this.show_progress(&p),
                    ProcessEvent::Stdout(line) => {
                        if let Some(w) = log.as_mut() {
                            let _ = w.write_line(&line);
                        }
                        this.log_line(&line);
                    }
                    ProcessEvent::Stderr(line) => {
                        if let Some(w) = log.as_mut() {
                            let _ = w.write_line(&line);
                        }
                        this.log_line(&format!("! {line}"));
                    }
                    ProcessEvent::Error(e) => {
                        if let Some(w) = log.as_mut() {
                            let _ = w.write_line(&format!("[error] {e}"));
                        }
                        error_summary = Some(e.clone());
                        this.log_line(&format!("[error] {e}"));
                    }
                    ProcessEvent::Finished { success, code } => {
                        exit_code = code;
                        failed = !success;
                        this.log_line(&format!("[finished success={success} code={code:?}]"));
                        break;
                    }
                }
            }
            let status = if failed { "failed" } else { "completed" };
            let _ = this
                .ctx
                .store
                .finish_run(run_id, status, exit_code, error_summary.as_deref());
            if let Some(w) = log.as_ref() {
                let _ = this.ctx.store.insert_run_log(
                    run_id,
                    &w.path().to_string_lossy(),
                    &w.counts_json(),
                );
            }
            *this.current.borrow_mut() = None;
            this.set_running(false);
            if !failed {
                this.progress_bar.set_fraction(1.0);
            }
            this.notify_done(&job_name, !failed);
            (this.on_changed)();
        });
    }

    /// Render a progress snapshot onto the bar + label.
    fn show_progress(&self, p: &cascade_core::job::Progress) {
        match p.percent {
            Some(pct) => self
                .progress_bar
                .set_fraction((pct as f64 / 100.0).clamp(0.0, 1.0)),
            None => self.progress_bar.pulse(),
        }
        let mut parts = Vec::new();
        if let Some(pct) = p.percent {
            parts.push(format!("{pct:.0}%"));
        }
        if let Some(s) = p.speed_bps {
            parts.push(fmt_speed(s));
        }
        if let Some(eta) = p.eta_secs {
            parts.push(format!("ETA {}", fmt_duration(eta)));
        }
        self.progress_label.set_label(&parts.join("  ·  "));
    }

    /// Send a desktop notification when a run finishes.
    fn notify_done(&self, job_name: &str, ok: bool) {
        if let Some(app) = self.window.application() {
            let title = if ok { "Job completed" } else { "Job failed" };
            let notif = gio::Notification::new(title);
            notif.set_body(Some(job_name));
            app.send_notification(Some("cascade-run-finished"), &notif);
        }
    }

    fn set_running(&self, running: bool) {
        self.run_btn.set_sensitive(!running);
        self.dry_btn.set_sensitive(!running);
        self.save_btn.set_sensitive(!running);
        self.cancel_btn.set_sensitive(running);
        if running {
            self.progress_bar.set_fraction(0.0);
            self.progress_bar.set_visible(true);
            self.progress_label.set_label("starting…");
            self.progress_label.set_visible(true);
        }
    }

    fn clear_log(&self) {
        self.log_buffer.set_text("");
    }

    fn log_line(&self, text: &str) {
        let mut end = self.log_buffer.end_iter();
        self.log_buffer.insert(&mut end, text);
        self.log_buffer.insert(&mut end, "\n");
        let mark = self
            .log_buffer
            .create_mark(None, &self.log_buffer.end_iter(), false);
        self.log_view.scroll_mark_onscreen(&mark);
    }
}

/// Add folder- and file-picker buttons as suffixes on `row`. The folder picker
/// covers directory transfers; the file picker covers single-file transfers
/// (e.g. one large database dump).
fn connect_browse(inputs: &Rc<Inputs>, row: &adw::EntryRow) {
    let folder = browse_button();
    folder.set_tooltip_text(Some("Choose a folder"));
    let file = gtk::Button::from_icon_name("text-x-generic-symbolic");
    file.add_css_class("flat");
    file.set_valign(gtk::Align::Center);
    file.set_tooltip_text(Some("Choose a single file"));
    row.add_suffix(&file);
    row.add_suffix(&folder);

    {
        let this = inputs.clone();
        let target = row.clone();
        folder.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Select folder").build();
            let entry = target.clone();
            let inner = this.clone();
            dialog.select_folder(Some(&this.window), gio::Cancellable::NONE, move |res| {
                if let Ok(f) = res {
                    if let Some(p) = f.path() {
                        entry.set_text(&p.to_string_lossy());
                        inner.refresh_preview();
                    }
                }
            });
        });
    }
    {
        let this = inputs.clone();
        let target = row.clone();
        file.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Select file").build();
            let entry = target.clone();
            let inner = this.clone();
            dialog.open(Some(&this.window), gio::Cancellable::NONE, move |res| {
                if let Ok(f) = res {
                    if let Some(p) = f.path() {
                        entry.set_text(&p.to_string_lossy());
                        inner.refresh_preview();
                    }
                }
            });
        });
    }
}

fn kind_str(tool: Tool) -> &'static str {
    match tool {
        Tool::Rclone => "rclone",
        Tool::Rsync => "rsync",
    }
}

fn op_str(op: OpKind) -> &'static str {
    match op {
        OpKind::Copy => "copy",
        OpKind::Sync => "sync",
        OpKind::Move => "move",
    }
}

fn last_component(p: &str) -> String {
    let t = p.trim_end_matches('/');
    t.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(t)
        .to_string()
}

/// Human-readable transfer rate from bytes/second.
fn fmt_speed(bps: u64) -> String {
    const UNITS: [&str; 5] = ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"];
    let mut v = bps as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// Format a duration in seconds as `M:SS` or `H:MM:SS`.
fn fmt_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}
