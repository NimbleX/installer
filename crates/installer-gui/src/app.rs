//! Top-level window construction and screen routing.

use crate::screens::{ScreenDestination, ScreenInstall};
use crate::state::AppState;
use gtk4::prelude::*;
use gtk4::{
    gdk, glib, Application, ApplicationWindow, CssProvider, Settings, Stack, StackTransitionType,
};
use installer_core::Bootloader;
use std::cell::RefCell;
use std::rc::Rc;

pub const STACK_DEST: &str = "destination";
pub const STACK_INSTALL: &str = "install";

pub fn on_activate(app: &Application, bootloader: Bootloader) {
    install_css();
    prefer_dark();

    let state = Rc::new(RefCell::new(AppState {
        bootloader,
        ..AppState::default()
    }));

    let stack = Stack::builder()
        .transition_type(StackTransitionType::SlideLeftRight)
        .transition_duration(220)
        .build();

    let dest = ScreenDestination::new(stack.clone(), state.clone());
    let install = ScreenInstall::new(stack.clone(), state.clone());

    stack.add_named(dest.widget(), Some(STACK_DEST));
    stack.add_named(install.widget(), Some(STACK_INSTALL));
    stack.set_visible_child_name(STACK_DEST);

    let dest_rc = dest.clone();
    let install_rc = install.clone();
    stack.connect_visible_child_name_notify(move |s| {
        match s.visible_child_name().map(|n| n.to_string()).as_deref() {
            Some(STACK_DEST) => dest_rc.refresh_async(),
            Some(STACK_INSTALL) => install_rc.start(),
            _ => {}
        }
    });

    let win = ApplicationWindow::builder()
        .application(app)
        .title("NimbleX Installer")
        .default_width(960)
        .default_height(640)
        .child(&stack)
        .build();
    win.add_css_class("nimblex-window");
    win.present();

    // Initial scan. Schedule it after presenting the window so the first frame
    // is visible before slower NTFS/ext usage probes run.
    let dest_initial = dest.clone();
    glib::idle_add_local_once(move || dest_initial.refresh_async());

    let _ = state;
}

fn install_css() {
    let provider = CssProvider::new();
    let combined = format!(
        "{}\n{}",
        include_str!("../assets/tokens.css"),
        include_str!("../assets/style.css")
    );
    provider.load_from_string(&combined);
    if let Some(display) = gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    } else {
        glib::g_warning!("nimblex", "no default display; CSS not loaded");
    }
}

fn prefer_dark() {
    if let Some(settings) = Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
    }
}
