mod blocks;
mod bridge;
mod dialogs;
mod files;
mod i18n;
mod jobs;
mod params_view;
mod report;
mod window;

use adw::prelude::*;
use ampghost_engine::APP_ID;
use gtk::{gio, glib};

fn main() -> glib::ExitCode {
    init_tracing();
    i18n::init();
    if let Err(error) = gio::resources_register_include!("ampghost.gresource") {
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
    close_on_signals(&app);
    app.run()
}

fn close_on_signals(app: &adw::Application) {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let app = app.downgrade();
        glib_unix::unix_signal_add_local(signal, move || {
            let Some(app) = app.upgrade() else {
                return glib::ControlFlow::Break;
            };
            tracing::info!(signal, "signal received, closing");
            let windows = app.windows();
            if windows.is_empty() {
                app.quit();
            }
            for window in windows {
                window.close();
            }
            glib::ControlFlow::Continue
        });
    }
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
