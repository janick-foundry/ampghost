#![forbid(unsafe_code)]

mod blocks;
mod bridge;
mod dialogs;
mod i18n;
mod params_view;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};
use katana_engine::APP_ID;

fn main() -> glib::ExitCode {
    init_tracing();
    i18n::init();
    if let Err(error) = gio::resources_register_include!("katana-gtk.gresource") {
        tracing::error!(%error, "registering resources failed");
        return glib::ExitCode::FAILURE;
    }
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        if let Err(error) = window::present(app) {
            tracing::error!("starting the window failed: {error:#}");
            app.quit();
        }
    });
    app.run()
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}
