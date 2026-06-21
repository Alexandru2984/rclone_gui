//! Profiles: saved job specs that can be loaded back into the New Job screen.

use std::rc::Rc;

use adw::prelude::*;

use cascade_core::job::JobSpec;

use crate::ctx::AppCtx;

#[derive(Clone)]
pub struct ProfilesView {
    root: gtk::Widget,
    list: gtk::ListBox,
    empty: gtk::Label,
    ctx: Rc<AppCtx>,
    /// Called when the user loads a profile into the New Job form.
    on_load: Rc<dyn Fn(JobSpec)>,
}

impl ProfilesView {
    pub fn new(ctx: Rc<AppCtx>, on_load: Rc<dyn Fn(JobSpec)>) -> Self {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(vec!["boxed-list".to_string()])
            .build();
        let empty = gtk::Label::builder()
            .label("No profiles yet — configure a job and press “Save profile”.")
            .css_classes(vec!["dim-label".to_string()])
            .margin_top(24)
            .build();

        let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
        column.append(&empty);
        column.append(&list);
        let clamp = adw::Clamp::builder()
            .maximum_size(720)
            .child(&column)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&clamp)
            .build();
        scroller.set_margin_top(18);
        scroller.set_margin_bottom(18);
        scroller.set_margin_start(12);
        scroller.set_margin_end(12);

        let view = Self {
            root: scroller.upcast(),
            list,
            empty,
            ctx,
            on_load,
        };
        view.refresh();
        view
    }

    pub fn widget(&self) -> &gtk::Widget {
        &self.root
    }

    pub fn refresh(&self) {
        self.list.remove_all();
        let profiles = self.ctx.store.list_profiles().unwrap_or_default();
        self.empty.set_visible(profiles.is_empty());
        self.list.set_visible(!profiles.is_empty());

        for p in profiles {
            let row = adw::ActionRow::builder()
                .title(&p.name)
                .subtitle(&p.spec.preview().unwrap_or_default())
                .build();
            row.add_css_class("property");

            let load = gtk::Button::builder()
                .label("Load")
                .valign(gtk::Align::Center)
                .css_classes(vec!["flat".to_string()])
                .build();
            {
                let on_load = self.on_load.clone();
                let spec = p.spec.clone();
                load.connect_clicked(move |_| on_load(spec.clone()));
            }

            let delete = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(vec!["flat".to_string()])
                .tooltip_text("Delete profile")
                .build();
            {
                let this = self.clone();
                let id = p.id;
                delete.connect_clicked(move |_| {
                    let _ = this.ctx.store.delete_profile(id);
                    this.refresh();
                });
            }

            row.add_suffix(&load);
            row.add_suffix(&delete);
            self.list.append(&row);
        }
    }
}
