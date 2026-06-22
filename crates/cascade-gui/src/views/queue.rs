//! Jobs Queue: enqueue jobs and run up to `max_parallel` (from Settings) at
//! once. Each job shows live status/progress and can be cancelled; finished
//! rows can be cleared.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;

use cascade_core::job::{JobSpec, Queue};
use cascade_core::logs::LogWriter;
use cascade_core::process::{progress, spawn_with_parser, LineParser, ProcessEvent, RunHandle};
use cascade_core::Tool;

use crate::ctx::AppCtx;

struct Item {
    spec: JobSpec,
    row: adw::ActionRow,
    cancel: gtk::Button,
    handle: Option<RunHandle>,
    done: bool,
}

#[derive(Clone)]
pub struct QueueView {
    root: gtk::Widget,
    ctx: Rc<AppCtx>,
    list: gtk::ListBox,
    empty: gtk::Label,
    queue: Rc<RefCell<Queue<u64>>>,
    items: Rc<RefCell<HashMap<u64, Item>>>,
    next_id: Rc<Cell<u64>>,
    on_changed: Rc<dyn Fn()>,
}

impl QueueView {
    pub fn new(ctx: Rc<AppCtx>, on_changed: Rc<dyn Fn()>) -> Self {
        let max = ctx.settings.borrow().max_parallel.max(1) as usize;

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list".to_string()])
            .build();
        let empty = gtk::Label::builder()
            .label("Queue is empty. Add jobs from “New Job → Add to queue”.")
            .css_classes(vec!["dim-label".to_string()])
            .margin_top(24)
            .build();

        let clear = gtk::Button::builder()
            .label("Clear finished")
            .halign(gtk::Align::End)
            .css_classes(vec!["pill".to_string()])
            .build();

        let group = adw::PreferencesGroup::builder().title("Jobs").build();
        group.set_header_suffix(Some(&clear));
        group.add(&empty);
        group.add(&list);

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.set_margin_top(16);
        column.set_margin_bottom(16);
        column.set_margin_start(12);
        column.set_margin_end(12);
        column.append(&group);

        let clamp = adw::Clamp::builder()
            .maximum_size(760)
            .child(&column)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();

        let view = Self {
            root: scroller.upcast(),
            ctx,
            list,
            empty,
            queue: Rc::new(RefCell::new(Queue::new(max))),
            items: Rc::new(RefCell::new(HashMap::new())),
            next_id: Rc::new(Cell::new(1)),
            on_changed,
        };

        {
            let this = view.clone();
            clear.connect_clicked(move |_| this.clear_finished());
        }
        view.refresh_empty();
        view
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.root
    }

    /// Add a job to the queue and try to start it.
    pub fn enqueue(&self, spec: JobSpec) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);

        let row = adw::ActionRow::builder()
            .title(&spec.name)
            .subtitle("queued")
            .build();
        let cancel = gtk::Button::builder()
            .icon_name("process-stop-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(vec!["flat".to_string()])
            .tooltip_text("Cancel")
            .sensitive(false)
            .build();
        {
            let this = self.clone();
            cancel.connect_clicked(move |_| this.cancel_item(id));
        }
        row.add_suffix(&cancel);
        self.list.append(&row);

        self.items.borrow_mut().insert(
            id,
            Item {
                spec,
                row,
                cancel,
                handle: None,
                done: false,
            },
        );
        self.queue.borrow_mut().enqueue(id);
        self.refresh_empty();
        self.pump();
    }

    fn pump(&self) {
        let max = self.ctx.settings.borrow().max_parallel.max(1) as usize;
        self.queue.borrow_mut().set_max(max);
        let ready = self.queue.borrow_mut().start_ready();
        for id in ready {
            self.launch(id);
        }
    }

    fn launch(&self, id: u64) {
        let (spec, row, cancel) = match self.items.borrow().get(&id) {
            Some(it) => (it.spec.clone(), it.row.clone(), it.cancel.clone()),
            None => return,
        };

        let argv = match spec.build_argv() {
            Ok(a) => a,
            Err(e) => {
                row.set_subtitle(&format!("error: {e}"));
                self.mark_done(id);
                self.queue.borrow_mut().complete();
                self.pump();
                return;
            }
        };
        let preview = spec.preview().unwrap_or_default();

        let options_json = serde_json::to_string(&spec).unwrap_or_else(|_| "{}".into());
        let job_id = match self.ctx.store.insert_job(
            &spec.name,
            kind_str(spec.tool),
            op_str(&spec),
            &spec.source,
            &spec.destination,
            &options_json,
        ) {
            Ok(j) => j,
            Err(e) => {
                row.set_subtitle(&format!("database error: {e}"));
                self.mark_done(id);
                self.queue.borrow_mut().complete();
                self.pump();
                return;
            }
        };
        let run_id = self
            .ctx
            .store
            .start_run(job_id, spec.dry_run, &preview)
            .unwrap_or(-1);

        row.set_subtitle("running…");
        cancel.set_sensitive(true);

        let parser: LineParser = match spec.tool {
            Tool::Rsync => Arc::new(progress::parse_rsync),
            Tool::Rclone => Arc::new(progress::parse_rclone),
        };
        let handle = spawn_with_parser(spec.binary(), argv, Some(parser));
        let events = handle.events.clone();
        if let Some(it) = self.items.borrow_mut().get_mut(&id) {
            it.handle = Some(handle);
        }

        let this = self.clone();
        let log_dir = self.ctx.paths.log_dir.clone();
        glib::spawn_future_local(async move {
            let mut log = LogWriter::create(&log_dir, run_id).ok();
            if let Some(w) = log.as_mut() {
                let _ = w.write_line(&format!("$ {preview}"));
            }
            let mut exit_code = None;
            let mut failed = false;
            let mut error_summary: Option<String> = None;
            while let Ok(ev) = events.recv().await {
                match ev {
                    ProcessEvent::Started { .. } => {}
                    ProcessEvent::Progress(p) => row.set_subtitle(&fmt_progress(&p)),
                    ProcessEvent::Stdout(l) | ProcessEvent::Stderr(l) => {
                        if let Some(w) = log.as_mut() {
                            let _ = w.write_line(&l);
                        }
                    }
                    ProcessEvent::Error(e) => {
                        error_summary = Some(e.clone());
                        if let Some(w) = log.as_mut() {
                            let _ = w.write_line(&format!("[error] {e}"));
                        }
                    }
                    ProcessEvent::Finished { success, code } => {
                        exit_code = code;
                        failed = !success;
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
            row.set_subtitle(status);
            cancel.set_sensitive(false);
            this.mark_done(id);
            this.queue.borrow_mut().complete();
            this.pump();
            (this.on_changed)();
        });
    }

    fn cancel_item(&self, id: u64) {
        if let Some(it) = self.items.borrow().get(&id) {
            if let Some(handle) = &it.handle {
                handle.cancel();
                it.row.set_subtitle("cancelling…");
            }
        }
    }

    fn mark_done(&self, id: u64) {
        if let Some(it) = self.items.borrow_mut().get_mut(&id) {
            it.handle = None;
            it.done = true;
        }
    }

    fn clear_finished(&self) {
        let done_ids: Vec<u64> = self
            .items
            .borrow()
            .iter()
            .filter(|(_, it)| it.done)
            .map(|(id, _)| *id)
            .collect();
        for id in done_ids {
            if let Some(it) = self.items.borrow_mut().remove(&id) {
                self.list.remove(&it.row);
            }
        }
        self.refresh_empty();
    }

    fn refresh_empty(&self) {
        let empty = self.items.borrow().is_empty();
        self.empty.set_visible(empty);
        self.list.set_visible(!empty);
    }
}

fn kind_str(tool: Tool) -> &'static str {
    match tool {
        Tool::Rclone => "rclone",
        Tool::Rsync => "rsync",
    }
}

fn op_str(spec: &JobSpec) -> &'static str {
    use cascade_core::job::OpKind::*;
    match spec.op {
        Copy => "copy",
        Sync => "sync",
        Move => "move",
    }
}

fn fmt_progress(p: &cascade_core::job::Progress) -> String {
    let mut s = String::from("running");
    if let Some(pct) = p.percent {
        s.push_str(&format!(" · {pct:.0}%"));
    }
    if let Some(bps) = p.speed_bps {
        s.push_str(&format!(" · {}", fmt_speed(bps)));
    }
    s
}

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
