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
    init_tracing();

    // Parse CLI args before constructing the GTK app. Use `try_parse_from`
    // with our own argv so GTK doesn't see the flags. We pass an empty
    // argv to GTK below to keep both happy.
    let cli = Cli::parse();

    let app = gtk4::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::FLAGS_NONE)
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
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .try_init();
}
