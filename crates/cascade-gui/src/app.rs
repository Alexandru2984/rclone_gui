//! Application bootstrap: create the AdwApplication and present the main window.

use adw::prelude::*;
use adw::Application;

use cascade_core::config::{APP_ID, APP_NAME};

use crate::ctx::{apply_theme, AppCtx};
use crate::window::MainWindow;

pub fn run() -> glib::ExitCode {
    init_logging();
    crate::i18n::init();
    let ctx = AppCtx::new();

    // Set a human-readable application name. In GTK4 the window icon and the
    // desktop-notification identity are resolved from the installed .desktop
    // file matched by the GApplication id (works on both X11 and Wayland), so
    // there is no per-window icon API to call here.
    glib::set_application_name(APP_NAME);

    let app = Application::builder().application_id(APP_ID).build();

    app.connect_activate(move |app| {
        apply_theme(ctx.settings.borrow().theme);
        let window = MainWindow::build(app, ctx.clone());
        window.present();
    });

    // Do not forward process argv to GTK; we have no GTK CLI options.
    app.run_with_args::<&str>(&[])
}

/// Initialize structured logging. Override with `RUST_LOG`, e.g.
/// `RUST_LOG=cascade_core=debug`.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,cascade_gui=info,cascade_core=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
