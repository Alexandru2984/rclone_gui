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
    up: gtk::Button,
    down: gtk::Button,
    remove: gtk::Button,
    cancel: gtk::Button,
    handle: Option<RunHandle>,
    done: bool,
    /// True once the job has been launched; launched/running jobs are not
    /// persisted, so a crash mid-run won't silently re-queue them.
    launched: bool,
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
    paused: Rc<Cell<bool>>,
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
            .label(crate::i18n::tr(
                "Queue is empty. Add jobs from “New Job → Add to queue”.",
            ))
            .css_classes(vec!["dim-label".to_string()])
            .margin_top(24)
            .build();

        let pause = gtk::Button::builder()
            .label(crate::i18n::tr("Pause"))
            .css_classes(vec!["pill".to_string()])
            .build();
        let clear = gtk::Button::builder()
            .label(crate::i18n::tr("Clear finished"))
            .css_classes(vec!["pill".to_string()])
            .build();
        let header_buttons = gtk::Box::builder().spacing(6).build();
        header_buttons.append(&pause);
        header_buttons.append(&clear);

        let group = adw::PreferencesGroup::builder()
            .title(crate::i18n::tr("Jobs"))
            .build();
        group.set_header_suffix(Some(&header_buttons));
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
            paused: Rc::new(Cell::new(false)),
            on_changed,
        };

        {
            let this = view.clone();
            clear.connect_clicked(move |_| this.clear_finished());
        }
        {
            let this = view.clone();
            pause.connect_clicked(move |btn| {
                let now_paused = !this.paused.get();
                this.paused.set(now_paused);
                btn.set_label(&if now_paused {
                    crate::i18n::tr("Resume")
                } else {
                    crate::i18n::tr("Pause")
                });
                if !now_paused {
                    this.pump();
                }
            });
        }
        view.refresh_empty();
        // Restore a queue left pending from a previous session, but start it
        // PAUSED: a destructive job enqueued before the restart must not run
        // unattended without the user explicitly resuming it.
        let restored = view.ctx.store.queue_list().unwrap_or_default();
        if !restored.is_empty() {
            view.paused.set(true);
            pause.set_label(&crate::i18n::tr("Resume"));
            for spec in restored {
                view.enqueue(spec); // pump() is suppressed while paused
            }
        }
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
            .title(crate::views::esc(&spec.name))
            .subtitle(crate::i18n::tr("queued"))
            .build();

        let mk = |icon: &str, tip: &str| {
            gtk::Button::builder()
                .icon_name(icon)
                .valign(gtk::Align::Center)
                .css_classes(vec!["flat".to_string()])
                .tooltip_text(tip)
                .build()
        };
        let up = mk("go-up-symbolic", &crate::i18n::tr("Move up"));
        let down = mk("go-down-symbolic", &crate::i18n::tr("Move down"));
        let remove = mk(
            "list-remove-symbolic",
            &crate::i18n::tr("Remove from queue"),
        );
        let cancel = mk("process-stop-symbolic", &crate::i18n::tr("Cancel"));
        cancel.set_visible(false); // shown only while running

        for b in [&up, &down, &remove, &cancel] {
            row.add_suffix(b);
        }
        {
            let this = self.clone();
            up.connect_clicked(move |_| this.move_item(id, -1));
        }
        {
            let this = self.clone();
            down.connect_clicked(move |_| this.move_item(id, 1));
        }
        {
            let this = self.clone();
            remove.connect_clicked(move |_| this.remove_item(id));
        }
        {
            let this = self.clone();
            cancel.connect_clicked(move |_| this.cancel_item(id));
        }
        self.list.append(&row);

        self.items.borrow_mut().insert(
            id,
            Item {
                spec,
                row,
                up,
                down,
                remove,
                cancel,
                handle: None,
                done: false,
                launched: false,
            },
        );
        self.queue.borrow_mut().enqueue(id);
        self.refresh_empty();
        self.persist();
        self.pump();
    }

    /// Mirror the still-pending (not yet launched) jobs to the store, in visual
    /// order, so the queue survives a restart. Secret-bearing specs are skipped
    /// — Cascade never persists credentials.
    fn persist(&self) {
        let items = self.items.borrow();
        let mut pending: Vec<(i32, JobSpec)> = items
            .values()
            .filter(|it| !it.launched && !it.done && !it.spec.contains_secret())
            .map(|it| (it.row.index(), it.spec.clone()))
            .collect();
        pending.sort_by_key(|(idx, _)| *idx);
        let specs: Vec<JobSpec> = pending.into_iter().map(|(_, s)| s).collect();
        if let Err(e) = self.ctx.store.queue_replace(&specs) {
            tracing::warn!("could not persist the queue: {e}");
        }
    }

    fn pump(&self) {
        if self.paused.get() {
            return;
        }
        let max = self.ctx.settings.borrow().max_parallel.max(1) as usize;
        self.queue.borrow_mut().set_max(max);
        let ready = self.queue.borrow_mut().start_ready();
        for id in ready {
            self.launch(id);
        }
    }

    /// Remove a still-queued job (no effect once running).
    fn remove_item(&self, id: u64) {
        if self.queue.borrow_mut().remove(&id) {
            if let Some(it) = self.items.borrow_mut().remove(&id) {
                self.list.remove(&it.row);
            }
            self.refresh_empty();
            self.persist();
        }
    }

    /// Reorder a still-queued job by one position (`dir` = -1 up, +1 down).
    fn move_item(&self, id: u64, dir: i32) {
        let moved = if dir < 0 {
            self.queue.borrow_mut().move_up(&id)
        } else {
            self.queue.borrow_mut().move_down(&id)
        };
        if !moved {
            return;
        }
        if let Some(it) = self.items.borrow().get(&id) {
            let idx = it.row.index();
            let target = idx + dir;
            if target >= 0 {
                self.list.remove(&it.row);
                self.list.insert(&it.row, target);
            }
        }
        self.persist();
    }

    fn launch(&self, id: u64) {
        let (spec, row, cancel) = match self.items.borrow().get(&id) {
            Some(it) => (it.spec.clone(), it.row.clone(), it.cancel.clone()),
            None => return,
        };
        // Once launched, the job is no longer pending: mark it and drop it from
        // the persisted queue (so a crash mid-run won't re-queue it).
        if let Some(it) = self.items.borrow_mut().get_mut(&id) {
            it.launched = true;
        }
        self.persist();

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
        // Sanitized: persisted to the DB and written to the on-disk log.
        let preview = spec.preview_sanitized().unwrap_or_default();

        let job_id = match self.ctx.store.insert_job(&spec) {
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

        row.set_subtitle(&crate::i18n::tr("running…"));
        // Switch the row controls from queued (reorder/remove) to running (cancel).
        if let Some(it) = self.items.borrow().get(&id) {
            it.up.set_visible(false);
            it.down.set_visible(false);
            it.remove.set_visible(false);
            it.cancel.set_visible(true);
        }
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
            row.set_subtitle(&crate::i18n::tr(status));
            cancel.set_visible(false);
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
                it.row.set_subtitle(&crate::i18n::tr("cancelling…"));
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

fn fmt_progress(p: &cascade_core::job::Progress) -> String {
    let mut s = String::from("running");
    if let Some(pct) = p.percent {
        s.push_str(&format!(" · {pct:.0}%"));
    }
    if let Some(total) = p.bytes_total {
        s.push_str(&format!(
            " · {} / {}",
            fmt_bytes(p.bytes_transferred),
            fmt_bytes(total)
        ));
    }
    if let Some(bps) = p.speed_bps {
        s.push_str(&format!(" · {}", fmt_speed(bps)));
    }
    s
}

fn fmt_speed(bps: u64) -> String {
    format!("{}/s", fmt_bytes(bps))
}

fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}
