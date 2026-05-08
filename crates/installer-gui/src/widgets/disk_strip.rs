//! Custom widget that renders a disk as a horizontal segmented bar,
//! the way MiniTool / EaseUS / GParted do. Each partition is a coloured
//! rectangle proportional to its size, with its label and capacity
//! drawn inside when the segment is wide enough.
//!
//! Built on `gtk::DrawingArea` with a Cairo callback. We do not use the
//! `subclass` machinery to keep this dependency-light.

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::DrawingArea;
use installer_core::{Bytes, Disk, Partition, PartitionRole};
use std::cell::RefCell;
use std::rc::Rc;

const STRIP_HEIGHT: i32 = 72;

#[derive(Clone)]
pub struct DiskStripView {
    area: DrawingArea,
    state: Rc<RefCell<StripState>>,
}

struct StripState {
    disk: Option<Disk>,
    selected_partition: Option<u32>, // partition number
}

impl DiskStripView {
    pub fn new() -> Self {
        let area = DrawingArea::builder()
            .height_request(STRIP_HEIGHT)
            .hexpand(true)
            .content_height(STRIP_HEIGHT)
            .build();
        let state = Rc::new(RefCell::new(StripState {
            disk: None,
            selected_partition: None,
        }));
        let state_draw = state.clone();
        area.set_draw_func(move |_a, cr, w, h| {
            if let Some(disk) = state_draw.borrow().disk.as_ref() {
                draw_strip(cr, w, h, disk, state_draw.borrow().selected_partition);
            } else {
                draw_empty(cr, w, h);
            }
        });
        Self { area, state }
    }

    pub fn widget(&self) -> &DrawingArea {
        &self.area
    }

    pub fn set_disk(&self, disk: Disk) {
        self.state.borrow_mut().disk = Some(disk);
        self.state.borrow_mut().selected_partition = None;
        self.area.queue_draw();
    }

    pub fn set_selected(&self, partition_number: Option<u32>) {
        self.state.borrow_mut().selected_partition = partition_number;
        self.area.queue_draw();
    }

    /// Hit-test: given a click x in widget pixels, return the partition
    /// number at that position, or `None` if the click was on free space.
    pub fn partition_at(&self, click_x: f64) -> Option<u32> {
        let s = self.state.borrow();
        let disk = s.disk.as_ref()?;
        let total = disk.size.0 as f64;
        let width = self.area.width() as f64;
        // Recompute the same segment layout the draw fn uses.
        let mut x_byte: u64 = 0;
        let bytes_per_pixel = total / width.max(1.0);
        let click_byte = (click_x * bytes_per_pixel) as u64;
        for p in &disk.partitions {
            let end = x_byte + p.size.0;
            if click_byte >= x_byte && click_byte < end {
                return Some(p.number);
            }
            x_byte = end;
        }
        None
    }
}

impl Default for DiskStripView {
    fn default() -> Self {
        Self::new()
    }
}

fn draw_empty(cr: &Context, w: i32, h: i32) {
    cr.set_source_rgb(0.93, 0.94, 0.96);
    cr.rectangle(0.0, 0.0, w as f64, h as f64);
    let _ = cr.fill();
}

fn draw_strip(cr: &Context, w: i32, h: i32, disk: &Disk, selected: Option<u32>) {
    let w = w as f64;
    let h = h as f64;
    // Outer frame.
    cr.set_source_rgb(0.82, 0.84, 0.87);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();

    let total = disk.size.0 as f64;
    if total <= 0.0 {
        return;
    }
    let mut x_byte: u64 = 0;
    let inner_y = 1.0;
    let inner_h = h - 2.0;
    for p in &disk.partitions {
        let frac_start = x_byte as f64 / total;
        let frac_end = (x_byte + p.size.0) as f64 / total;
        let x = frac_start * w;
        let segw = (frac_end - frac_start) * w;
        let (light, full) = role_color_pair(p.role);

        // 1. Full segment in the "light" tint = represents free space.
        cr.set_source_rgb(light.0, light.1, light.2);
        cr.rectangle(x + 0.5, inner_y, (segw - 1.0).max(0.0), inner_h);
        let _ = cr.fill();

        // 2. Used portion overlay in the "full" colour. If we don't know
        //    how much is used, paint diagonal stripes across the whole
        //    segment so it visually reads as "unknown" instead of a lie.
        let (used_frac, usage_known) = match p.used {
            Some(u) if p.size.0 > 0 => {
                ((u.0 as f64 / p.size.0 as f64).clamp(0.0, 1.0), true)
            }
            _ => (0.0, false),
        };
        if usage_known {
            let used_w = (segw * used_frac - 1.0).max(0.0);
            cr.set_source_rgb(full.0, full.1, full.2);
            cr.rectangle(x + 0.5, inner_y, used_w, inner_h);
            let _ = cr.fill();
        } else {
            draw_diagonal_stripes(
                cr,
                x + 0.5,
                inner_y,
                (segw - 1.0).max(0.0),
                inner_h,
                full,
            );
        }

        // 3. Selection highlight (drawn last so it sits on top).
        if Some(p.number) == selected {
            cr.set_source_rgb(1.0, 0.82, 0.27);
            cr.set_line_width(3.0);
            cr.rectangle(x + 1.5, inner_y + 1.5, (segw - 3.0).max(0.0), inner_h - 3.0);
            let _ = cr.stroke();
        }

        // 4. Lock icon for protected segments.
        if p.protected && segw > 18.0 {
            cr.set_source_rgb(0.3, 0.3, 0.35);
            let cx = x + segw - 12.0;
            let cy = inner_y + 8.0;
            cr.rectangle(cx, cy, 8.0, 6.0);
            let _ = cr.fill();
            cr.arc(cx + 4.0, cy, 3.0, std::f64::consts::PI, 0.0);
            cr.set_line_width(1.5);
            let _ = cr.stroke();
        }

        // 5. Text labels. Three lines if there's room: role, capacity,
        //    and a "used of total" usage line.
        if segw > 60.0 {
            cr.set_source_rgb(0.06, 0.08, 0.12);
            cr.move_to(x + 6.0, inner_y + 16.0);
            let _ = cr.show_text(p.role.short_label());
            cr.move_to(x + 6.0, inner_y + 32.0);
            let _ = cr.show_text(&format!("{}", p.size));
            if segw > 110.0 {
                let usage_line = if usage_known {
                    match p.used {
                        Some(u) => format!("{} used · {:.0}% full", u, used_frac * 100.0),
                        None => String::new(),
                    }
                } else {
                    "usage unknown".to_string()
                };
                cr.set_source_rgb(0.30, 0.32, 0.38);
                cr.move_to(x + 6.0, inner_y + 50.0);
                let _ = cr.show_text(&usage_line);
            }
        }
        x_byte += p.size.0;
    }
    // Trailing free (unallocated) space.
    if x_byte < disk.size.0 {
        let frac_start = x_byte as f64 / total;
        let x = frac_start * w;
        let segw = w - x;
        // Hatched-like appearance: light grey base with a slightly darker
        // diagonal-stripe tint (cheap visual cue without a real pattern).
        cr.set_source_rgb(0.90, 0.91, 0.93);
        cr.rectangle(x + 0.5, inner_y, (segw - 1.0).max(0.0), inner_h);
        let _ = cr.fill();
        if segw > 50.0 {
            cr.set_source_rgb(0.42, 0.45, 0.50);
            cr.move_to(x + 6.0, inner_y + 16.0);
            let _ = cr.show_text("Unallocated");
            cr.move_to(x + 6.0, inner_y + 32.0);
            let _ = cr.show_text(&format!("{}", Bytes(disk.size.0 - x_byte)));
        }
    }
    // Suppress warning in case Partition is unused for empty disks.
    let _ = std::any::type_name::<Partition>();
}

/// Each role returns `(light, full)`: the light tint paints the whole
/// segment ("free" portion) and the full colour overlays the used portion.
fn role_color_pair(role: PartitionRole) -> ((f64, f64, f64), (f64, f64, f64)) {
    match role {
        PartitionRole::WindowsSystem => ((0.85, 0.91, 1.00), (0.32, 0.55, 0.92)),
        PartitionRole::WindowsData => ((0.90, 0.93, 1.00), (0.55, 0.71, 0.96)),
        PartitionRole::EfiSystem => ((0.88, 0.95, 0.99), (0.42, 0.72, 0.92)),
        PartitionRole::MicrosoftReserved => ((0.92, 0.93, 0.96), (0.65, 0.68, 0.74)),
        PartitionRole::WindowsRecovery => ((0.90, 0.91, 0.94), (0.58, 0.62, 0.68)),
        PartitionRole::Linux => ((0.88, 0.96, 0.91), (0.32, 0.74, 0.45)),
        PartitionRole::LinuxSwap => ((0.96, 0.92, 0.84), (0.78, 0.58, 0.30)),
        PartitionRole::Other => ((0.93, 0.94, 0.96), (0.70, 0.73, 0.78)),
    }
}

/// Paint diagonal stripes across the rect using the role's "full" colour
/// at 35% opacity. Communicates "we don't know how full this is" without
/// requiring image assets.
fn draw_diagonal_stripes(
    cr: &Context,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    full: (f64, f64, f64),
) {
    if w <= 0.0 {
        return;
    }
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    let _ = cr.clip();
    cr.set_source_rgba(full.0, full.1, full.2, 0.35);
    cr.set_line_width(2.0);
    let step = 9.0;
    let mut i = -h;
    while i < w + h {
        cr.move_to(x + i, y);
        cr.line_to(x + i + h, y + h);
        i += step;
    }
    let _ = cr.stroke();
    let _ = cr.restore();
}
