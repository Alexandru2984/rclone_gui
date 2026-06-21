//! New Job: pick tool + operation + paths, preview the exact command, gate
//! destructive runs behind a confirmation, run it live, and persist to SQLite.
//!
//! This is the heart of Phase 1: a full, safe, end-to-end flow for both tools.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use cascade_core::job::{JobSpec, OpKind};
use cascade_core::process::{spawn, ProcessEvent};
use cascade_core::security::destructive::RiskLevel;
use cascade_core::security::path;
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

    preview: gtk::Label,
    risk: gtk::Label,

    log_view: gtk::TextView,
    log_buffer: gtk::TextBuffer,

    run_btn: gtk::Button,
    dry_btn: gtk::Button,

    /// Callback to refresh sibling screens (e.g. History) after a run.
    on_changed: Rc<dyn Fn()>,
}

/// Build the New Job screen. `on_changed` is invoked after a run completes so
/// the caller can refresh the History view.
pub fn build(
    ctx: Rc<AppCtx>,
    window: adw::ApplicationWindow,
    on_changed: Rc<dyn Fn()>,
) -> gtk::Widget {
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
    op.set_model(Some(&gtk::StringList::new(&["Copy", "Sync (mirror)", "Move"])));

    let job_group = adw::PreferencesGroup::builder().title("Job").build();
    job_group.add(&name);
    job_group.add(&tool);
    job_group.add(&op);

    let source = adw::EntryRow::builder().title("Source").text(format!("{}/", src.display())).build();
    let dest = adw::EntryRow::builder().title("Destination").text(format!("{}/", dst.display())).build();
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
    let cmd_group = adw::PreferencesGroup::builder().title("Command preview").build();
    cmd_group.add(&cmd_box);

    let log_view = gtk::TextView::builder().editable(false).monospace(true).build();
    let log_buffer = log_view.buffer();
    let scroller = gtk::ScrolledWindow::builder().min_content_height(200).vexpand(true).child(&log_view).build();
    let log_group = adw::PreferencesGroup::builder().title("Live output").build();
    log_group.add(&scroller);

    let dry_btn = gtk::Button::builder().label("Dry-run").css_classes(vec!["pill".to_string()]).build();
    let run_btn = gtk::Button::builder().label("Start").css_classes(vec!["pill".to_string(), "suggested-action".to_string()]).build();
    let btn_box = gtk::Box::builder().halign(gtk::Align::End).spacing(8).build();
    btn_box.append(&dry_btn);
    btn_box.append(&run_btn);

    let page = adw::PreferencesPage::new();
    page.add(&job_group);
    page.add(&paths_group);
    page.add(&opts_group);
    page.add(&cmd_group);
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
        preview,
        risk,
        log_view,
        log_buffer,
        run_btn,
        dry_btn,
        on_changed,
    });

    inputs.wire();
    inputs.refresh_preview();
    outer.upcast()
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

        let tool = if self.tool.selected() == 1 { Tool::Rclone } else { Tool::Rsync };
        let op = match self.op.selected() {
            1 => OpKind::Sync,
            2 => OpKind::Move,
            _ => OpKind::Copy,
        };
        let name = {
            let n = self.name.text().to_string();
            if n.trim().is_empty() {
                format!("{} {} → {}", op.label(), last_component(&source), last_component(&dest))
            } else {
                n
            }
        };

        Ok(JobSpec { name, tool, op, source, destination: dest, dry_run: false, delete: self.delete.is_active() })
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
            RiskLevel::Destructive => {
                ("⚠ Destructive — files at the destination may be deleted", "error")
            }
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
        if spec.risk().requires_confirmation() {
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
        dialog.add_responses(&[("cancel", "Cancel"), ("dry", "Dry-run first"), ("run", "Run anyway")]);
        dialog.set_response_appearance("run", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("dry"));
        dialog.set_close_response("cancel");

        let this = self.clone();
        dialog.choose(&self.window, gio::Cancellable::NONE, move |resp| match resp.as_str() {
            "run" => this.run(false),
            "dry" => this.run(true),
            _ => {}
        });
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
        let preview = spec.preview().unwrap_or_default();

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
        let run_id = self.ctx.store.start_run(job_id, spec.dry_run, &preview).unwrap_or(-1);

        self.clear_log();
        self.log_line(&format!("$ {preview}"));
        self.set_running(true);

        let handle = spawn(spec.binary(), argv);
        let events = handle.events.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let mut exit_code = None;
            let mut failed = false;
            let mut error_summary: Option<String> = None;
            while let Ok(ev) = events.recv().await {
                match ev {
                    ProcessEvent::Started { pid } => this.log_line(&format!("[started pid={pid:?}]")),
                    ProcessEvent::Stdout(line) => this.log_line(&line),
                    ProcessEvent::Stderr(line) => this.log_line(&format!("! {line}")),
                    ProcessEvent::Error(e) => {
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
            let _ = this.ctx.store.finish_run(run_id, status, exit_code, error_summary.as_deref());
            this.set_running(false);
            (this.on_changed)();
        });
    }

    fn set_running(&self, running: bool) {
        self.run_btn.set_sensitive(!running);
        self.dry_btn.set_sensitive(!running);
    }

    fn clear_log(&self) {
        self.log_buffer.set_text("");
    }

    fn log_line(&self, text: &str) {
        let mut end = self.log_buffer.end_iter();
        self.log_buffer.insert(&mut end, text);
        self.log_buffer.insert(&mut end, "\n");
        let mark = self.log_buffer.create_mark(None, &self.log_buffer.end_iter(), false);
        self.log_view.scroll_mark_onscreen(&mark);
    }
}

/// Add a folder-picker button as a suffix on `row` and open a native
/// folder chooser when it is clicked.
fn connect_browse(inputs: &Rc<Inputs>, row: &adw::EntryRow) {
    let button = browse_button();
    row.add_suffix(&button);
    let this = inputs.clone();
    let target = row.clone();
    button.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder().title("Select folder").build();
        let entry = target.clone();
        let inner = this.clone();
        dialog.select_folder(Some(&this.window), gio::Cancellable::NONE, move |res| {
            if let Ok(file) = res {
                if let Some(p) = file.path() {
                    entry.set_text(&p.to_string_lossy());
                    inner.refresh_preview();
                }
            }
        });
    });
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
    t.rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or(t).to_string()
}
