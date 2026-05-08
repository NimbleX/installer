//! Card shell + dynamic header/stepper.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Label, Orientation};

pub fn app_card() -> GtkBox {
    let card = GtkBox::new(Orientation::Vertical, 0);
    card.add_css_class("app-card");
    card.set_hexpand(true);
    card.set_vexpand(true);
    card
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderStep {
    Destination,
    Confirm,
    Install,
}

#[derive(Clone)]
pub struct Header {
    root: GtkBox,
    step_destination: Label,
    step_confirm: Label,
    step_install: Label,
    show_commands_btn: gtk4::Button,
}

impl Header {
    pub fn new(active: HeaderStep) -> Self {
        let root = GtkBox::new(Orientation::Horizontal, 12);
        root.add_css_class("app-header");

        let avatar = Label::new(Some("NX"));
        avatar.add_css_class("app-header-avatar");
        root.append(&avatar);

        let title = Label::new(Some("Nimblex Installer"));
        title.add_css_class("app-header-title");
        title.set_halign(Align::Start);
        title.set_hexpand(true);
        root.append(&title);

        let stepper = GtkBox::new(Orientation::Horizontal, 2);
        stepper.add_css_class("stepper");

        let step_destination = make_step("Destination");
        stepper.append(&step_destination);
        stepper.append(&make_dot());
        let step_confirm = make_step("Confirm");
        stepper.append(&step_confirm);
        stepper.append(&make_dot());
        let step_install = make_step("Install");
        stepper.append(&step_install);

        root.append(&stepper);

        let show_commands_btn = gtk4::Button::builder()
            .label(">_")
            .css_classes(["btn-secondary", "header-cmd-btn"])
            .tooltip_text("Show commands")
            .margin_start(16)
            .build();
        root.append(&show_commands_btn);

        let me = Self {
            root,
            step_destination,
            step_confirm,
            step_install,
            show_commands_btn,
        };
        me.set_active(active);
        me
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    pub fn show_commands_btn(&self) -> &gtk4::Button {
        &self.show_commands_btn
    }

    pub fn set_active(&self, active: HeaderStep) {
        for (label, step) in [
            (&self.step_destination, HeaderStep::Destination),
            (&self.step_confirm, HeaderStep::Confirm),
            (&self.step_install, HeaderStep::Install),
        ] {
            if step == active {
                label.add_css_class("active");
            } else {
                label.remove_css_class("active");
            }
        }
    }
}

fn make_step(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.add_css_class("stepper-step");
    label
}

fn make_dot() -> Label {
    let dot = Label::new(Some("•"));
    dot.add_css_class("stepper-dot");
    dot
}
