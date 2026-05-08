//! Disk selection card used by the Destination screen.
//!
//! Renders a wide tile with a circular icon badge on the left and two
//! lines of text on the right. The icon is a themed symbolic icon
//! (`drive-removable-media-symbolic` for USB, `drive-harddisk-symbolic`
//! for fixed disks, `drive-multidisk-symbolic` for arrays). The button
//! is a `ToggleButton` so a row of cards can be grouped into a radio
//! set via `set_group`.

use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Image, Label, Orientation, ToggleButton};
use installer_core::{Bytes, Disk};

fn icon_name_for(disk: &Disk) -> &'static str {
    if disk.removable || disk.transport.eq_ignore_ascii_case("usb") {
        "drive-removable-media-symbolic"
    } else if disk.transport.eq_ignore_ascii_case("nvme")
        || disk.transport.eq_ignore_ascii_case("sata")
        || disk.transport.eq_ignore_ascii_case("ata")
    {
        "drive-harddisk-symbolic"
    } else {
        "drive-multidisk-symbolic"
    }
}

/// Best-effort cleanup of the raw `lsblk` model string. Vendor part codes
/// like "SAMSUNG MZVL2512HCJQ-00BL7" become "SAMSUNG MZVL2512HCJQ" (drop
/// the trailing revision suffix). Empty strings fall back to a sensible
/// generic label per transport.
fn pretty_model(disk: &Disk) -> String {
    let trimmed = disk.model.trim();
    if trimmed.is_empty() {
        return if disk.removable || disk.transport.eq_ignore_ascii_case("usb") {
            "USB drive".into()
        } else if disk.transport.eq_ignore_ascii_case("nvme") {
            "NVMe SSD".into()
        } else {
            "Internal disk".into()
        };
    }
    // Drop revision tail ("-00BL7"-style: <=6 alphanumerics after a hyphen).
    match trimmed.rsplit_once('-') {
        Some((head, tail))
            if (1..=6).contains(&tail.len()) && tail.chars().all(|c| c.is_alphanumeric()) =>
        {
            head.trim().to_string()
        }
        _ => trimmed.to_string(),
    }
}

pub fn disk_card(disk: &Disk) -> ToggleButton {
    let btn = ToggleButton::new();
    btn.add_css_class("disk-card");
    btn.set_hexpand(true);

    let row = GtkBox::new(Orientation::Horizontal, 14);
    row.set_valign(Align::Center);

    let icon = Image::from_icon_name(icon_name_for(disk));
    icon.set_pixel_size(32);
    icon.add_css_class("disk-card-icon");
    row.append(&icon);

    // ---- Text block ----
    let text = GtkBox::new(Orientation::Vertical, 2);
    text.set_halign(Align::Start);
    text.set_valign(Align::Center);
    text.set_hexpand(true);

    // Title row: model + optional inline Windows pill.
    let title_row = GtkBox::new(Orientation::Horizontal, 8);
    title_row.set_halign(Align::Start);

    let title = Label::new(Some(&pretty_model(disk)));
    title.add_css_class("disk-card-title");
    title.set_halign(Align::Start);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(22);
    title_row.append(&title);

    if disk.has_windows() {
        let win_pill = Label::new(Some("🪟\u{FE0F} Windows"));
        win_pill.add_css_class("win-pill");
        win_pill.set_valign(Align::Center);
        title_row.append(&win_pill);
    }
    text.append(&title_row);

    // Meta: size · path. Transport already conveyed by the icon glyph.
    let meta = Label::new(Some(&format!(
        "{} · {}",
        Bytes(disk.size.0),
        disk.path.display()
    )));
    meta.add_css_class("disk-card-meta");
    meta.set_halign(Align::Start);
    meta.set_xalign(0.0);
    meta.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    meta.set_max_width_chars(28);
    text.append(&meta);

    // Mini usage bar
    let total_used: u64 = disk.partitions.iter().filter_map(|p| p.used.map(|b| b.0)).sum();
    let usage_fraction = if disk.size.0 > 0 {
        (total_used as f64) / (disk.size.0 as f64)
    } else {
        0.0
    };
    let usage_bar = gtk4::ProgressBar::new();
    usage_bar.set_fraction(usage_fraction.clamp(0.0, 1.0));
    usage_bar.add_css_class("disk-usage-bar");
    usage_bar.set_valign(Align::Center);
    usage_bar.set_margin_top(4);
    text.append(&usage_bar);

    row.append(&text);
    btn.set_child(Some(&row));
    btn
}
