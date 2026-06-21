//! Application bootstrap: create the AdwApplication and present the main window.

use adw::prelude::*;
use adw::Application;

use cascade_core::config::{Paths, APP_ID};

use crate::window::MainWindow;

pub fn run() -> glib::ExitCode {
    // Ensure data/config/log directories exist (private perms) before the UI starts.
    let paths = Paths::resolve();
    if let Err(e) = paths.ensure() {
        eprintln!("warning: could not create app directories: {e}");
    }

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| {
        let window = MainWindow::new(app);
        window.present();
    });

    // Do not forward process argv to GTK; we have no GTK CLI options.
    app.run_with_args::<&str>(&[])
}
