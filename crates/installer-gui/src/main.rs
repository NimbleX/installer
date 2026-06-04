//! Nimblex installer GUI (GTK4).
//!
//! A single-window, three-screen wizard. Navigation is a `gtk::Stack`; each
//! screen is its own module under `screens/`. The `state` module owns the
//! shared model the screens read/write (selected scenario, scanned disks,
//! chosen partition, splitter position, generated plan).

mod app;
mod screens;
mod state;
mod widgets;

use clap::Parser;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use installer_core::Bootloader;

const APP_ID: &str = "org.nimblex.Installer";

/// Nimblex installer.
#[derive(Parser, Debug)]
#[command(name = "nimblex-installer", version, about)]
struct Cli {
    /// Bootloader to install on the target. `auto` (the default) picks
    /// systemd-boot on UEFI hosts and GRUB on legacy BIOS hosts. Use
    /// `grub` to force GRUB on UEFI (e.g. when you need cross-ESP Windows
    /// chainloading from a USB live).
    #[arg(long, value_parser = parse_bootloader, default_value = "auto")]
    bootloader: Bootloader,
}

fn parse_bootloader(s: &str) -> Result<Bootloader, String> {
    s.parse::<Bootloader>()
}

fn main() -> glib::ExitCode {
    disable_session_integrations();
    init_tracing();
    suppress_session_integration_warnings();

    // Parse CLI args before constructing the GTK app. Use `try_parse_from`
    // with our own argv so GTK doesn't see the flags. We pass an empty
    // argv to GTK below to keep both happy.
    let cli = Cli::parse();

    let app = gtk4::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let bootloader = cli.bootloader;
    app.connect_activate(move |app| app::on_activate(app, bootloader));
    // Tell GTK to ignore the process argv (it would otherwise try to open
    // each arg as a file). Pass argv0 only.
    let argv0 = std::env::args().next().unwrap_or_default();
    app.run_with_args(&[argv0])
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init();
}

/// Avoid GTK/GDK startup stalls caused by broken live-session integrations.
///
/// NimbleX runs the installer as root. In that session the user systemd
/// manager may not have `DISPLAY`/`WAYLAND_DISPLAY` in its activation
/// environment, so D-Bus activation starts `xdg-desktop-portal-gtk` without a
/// display. GDK then waits for portal interfaces such as
/// `org.freedesktop.host.portal.Registry` until D-Bus times out. The installer
/// does not need portals, accessibility bus integration, or session-bus
/// single-instance behavior, so opt out before GTK initialises.
///
/// Set `NIMBLEX_INSTALLER_USE_SESSION_BUS=1` to keep the caller's session bus
/// for debugging.
fn disable_session_integrations() {
    if std::env::var_os("GTK_USE_PORTAL").is_none() {
        std::env::set_var("GTK_USE_PORTAL", "0");
    }
    if std::env::var_os("NO_AT_BRIDGE").is_none() {
        std::env::set_var("NO_AT_BRIDGE", "1");
    }
    if std::env::var_os("NIMBLEX_INSTALLER_USE_SESSION_BUS").is_none() {
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", "unix:path=/dev/null");
    }
}

/// Silence harmless warnings GTK logs on systems where session integrations are
/// not available or are intentionally disabled for the installer.
///
/// GDK emits these via GLib *structured* logging, so a plain
/// `log_set_handler` does not catch them — we must install a writer func.
fn suppress_session_integration_warnings() {
    glib::log_set_writer_func(|level, fields| {
        let handled_warning = fields.iter().any(|f| {
            f.key() == "MESSAGE"
                && f.value_str().is_some_and(|v| {
                    v.contains("portal.Inhibit")
                        || v.contains("portal.Registry")
                        || v.contains("org.freedesktop.portal")
                        || v.contains("org.freedesktop.host.portal")
                        || v.contains("Unable to acquire session bus")
                })
        });
        if handled_warning {
            return glib::LogWriterOutput::Handled;
        }
        glib::log_writer_default(level, fields)
    });
}
