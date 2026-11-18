// SPDX-License-Identifier: MIT OR Apache-2.0
//! zTop for Linux: a GTK4 and libadwaita window onto the fleet. The
//! logic lives in `ztop-desktop-core`; this crate is the window, the
//! dialogs and the threads that keep network calls off the UI.

mod app;
mod card;
mod dialogs;
mod hooks;
mod secret;

use adw::prelude::*;

pub const APP_ID: &str = "dev.ztop.Desktop";

fn main() -> gtk::glib::ExitCode {
    let application = adw::Application::builder().application_id(APP_ID).build();
    application.connect_startup(|_| app::load_css());
    application.connect_activate(app::activate);
    // GTK would read our hook variables' neighbours as its own flags.
    application.run_with_args::<&str>(&[])
}
