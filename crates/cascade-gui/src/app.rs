//! Application bootstrap: create the AdwApplication and present the main window.

use adw::prelude::*;
use adw::Application;

use cascade_core::config::APP_ID;

use crate::ctx::{apply_theme, AppCtx};
use crate::window::MainWindow;

pub fn run() -> glib::ExitCode {
    let ctx = AppCtx::new();

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| {
        apply_theme(ctx.settings.borrow().theme);
        let window = MainWindow::new(app, ctx.clone());
        window.present();
    });

    // Do not forward process argv to GTK; we have no GTK CLI options.
    app.run_with_args::<&str>(&[])
}
