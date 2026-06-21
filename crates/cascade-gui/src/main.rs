//! Cascade GUI entry point.
//!
//! Thin GTK4 / libadwaita layer over `cascade-core`. All business logic lives
//! in the core crate; this binary only builds widgets and bridges the core's
//! `async-channel` process events into the GLib main loop.

mod app;
mod ctx;
mod views;
mod window;

fn main() -> glib::ExitCode {
    app::run()
}
