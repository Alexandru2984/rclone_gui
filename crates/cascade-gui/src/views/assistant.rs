//! Backup Assistant: pick a scenario, fill source/destination with guidance,
//! then hand a preconfigured spec to the New Job screen (where the usual
//! dry-run / destructive-confirmation gates apply).

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use cascade_core::assistant::{builtin_scenarios, Scenario};
use cascade_core::job::JobSpec;
use cascade_core::security::destructive::RiskLevel;

/// Build the Assistant screen. `on_open` receives the configured spec and is
/// expected to load it into New Job and navigate there.
pub fn build(window: adw::ApplicationWindow, on_open: Rc<dyn Fn(JobSpec)>) -> gtk::Widget {
    let nav = adw::NavigationView::new();

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(vec!["boxed-list".to_string()])
        .build();

    for sc in builtin_scenarios() {
        let row = adw::ActionRow::builder()
            .title(sc.title)
            .subtitle(sc.description)
            .build();
        row.add_suffix(&risk_tag(sc.risk()));
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        row.set_activatable(true);

        let nav = nav.clone();
        let window = window.clone();
        let on_open = on_open.clone();
        row.connect_activated(move |_| {
            nav.push(&detail_page(sc, window.clone(), on_open.clone()));
        });
        list.append(&row);
    }

    let intro = adw::PreferencesGroup::builder()
        .title("Backup Assistant")
        .description("Pick a scenario and we'll set up the job for you")
        .build();
    intro.add(&list);

    let page = adw::PreferencesPage::new();
    page.add(&intro);

    let list_page = adw::NavigationPage::new(&page, "Backup Assistant");
    nav.add(&list_page);
    nav.upcast()
}

fn detail_page(
    sc: Scenario,
    window: adw::ApplicationWindow,
    on_open: Rc<dyn Fn(JobSpec)>,
) -> adw::NavigationPage {
    let source = adw::EntryRow::builder()
        .title("Source")
        .text(sc.source_hint)
        .build();
    let dest = adw::EntryRow::builder()
        .title("Destination")
        .text(sc.dest_hint)
        .build();
    add_browse(&source, &window);
    add_browse(&dest, &window);

    let paths = adw::PreferencesGroup::builder()
        .title("Paths")
        .description(sc.description)
        .build();
    paths.add(&source);
    paths.add(&dest);

    let group = adw::PreferencesGroup::new();
    if !sc.note.is_empty() {
        let note = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .label(sc.note)
            .build();
        note.add_css_class(match sc.risk() {
            RiskLevel::Destructive => "error",
            RiskLevel::Caution => "warning",
            RiskLevel::Safe => "dim-label",
        });
        group.add(&note);
    }

    let open = gtk::Button::builder()
        .label("Open in New Job")
        .halign(gtk::Align::End)
        .css_classes(vec!["pill".to_string(), "suggested-action".to_string()])
        .build();
    {
        let source = source.clone();
        let dest = dest.clone();
        open.connect_clicked(move |_| {
            let spec = sc.to_spec(&source.text(), &dest.text());
            on_open(spec);
        });
    }

    let page = adw::PreferencesPage::new();
    page.add(&paths);
    page.add(&group);

    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.append(&page);
    let btn_row = gtk::Box::builder()
        .halign(gtk::Align::End)
        .margin_end(12)
        .margin_bottom(12)
        .build();
    btn_row.append(&open);
    column.append(&btn_row);

    adw::NavigationPage::new(&column, sc.title)
}

fn add_browse(row: &adw::EntryRow, window: &adw::ApplicationWindow) {
    let button = gtk::Button::from_icon_name("folder-open-symbolic");
    button.add_css_class("flat");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some("Choose a local folder"));
    row.add_suffix(&button);

    let entry = row.clone();
    let window = window.clone();
    button.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder().title("Select folder").build();
        let entry = entry.clone();
        dialog.select_folder(Some(&window), gio::Cancellable::NONE, move |res| {
            if let Ok(file) = res {
                if let Some(p) = file.path() {
                    entry.set_text(&p.to_string_lossy());
                }
            }
        });
    });
}

fn risk_tag(risk: RiskLevel) -> gtk::Label {
    let (text, css) = match risk {
        RiskLevel::Safe => ("safe", "success"),
        RiskLevel::Caution => ("caution", "warning"),
        RiskLevel::Destructive => ("destructive", "error"),
    };
    gtk::Label::builder()
        .label(text)
        .valign(gtk::Align::Center)
        .css_classes(vec!["caption".to_string(), css.to_string()])
        .build()
}
