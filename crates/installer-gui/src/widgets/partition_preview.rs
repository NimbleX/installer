//! "Planned state" partition preview.
//!
//! A single horizontal segmented strip showing the post-install layout.
//! Small partitions are guaranteed a minimum visual width so nothing collapses
//! to a pixel. No text is drawn inside the strip — a colour-coded legend line
//! below the strip explains every segment.

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, DrawingArea, GestureDrag, Label, Orientation};
use installer_core::{Bytes, Disk, PartitionRole};
use std::cell::RefCell;
use std::rc::Rc;

const STRIP_HEIGHT: i32 = 56;
const FRAME_RADIUS: f64 = 10.0;
/// Minimum visual width (px) for any segment, regardless of byte proportion.
const MIN_SEG_PX: f64 = 30.0;

/// Action verb shown in the legend for a segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentAction {
    Keep,
    Resize,
    NewNimblex,
    NewEsp,
    Free,
}

impl SegmentAction {
    fn legend_tag(self) -> &'static str {
        match self {
            SegmentAction::Keep => "",
            SegmentAction::Resize => " → resized",
            SegmentAction::NewNimblex => " new",
            SegmentAction::NewEsp => " new",
            SegmentAction::Free => "",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlannedSegment {
    pub start: u64,
    pub size: u64,
    pub role: PartitionRole,
    pub label: String,
    pub action: SegmentAction,
}

#[derive(Clone)]
pub struct PartitionPreview {
    root: GtkBox,
    preview_area: DrawingArea,
    legend: Label,
    state: Rc<RefCell<PreviewState>>,
}

#[derive(Default)]
struct PreviewState {
    disk: Option<Disk>,
    planned: Option<Vec<PlannedSegment>>,
    planned_total: u64,
}

impl PartitionPreview {
    pub fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 6);
        root.add_css_class("preview-stack");

        let preview_area = DrawingArea::builder()
            .height_request(STRIP_HEIGHT)
            .hexpand(true)
            .content_height(STRIP_HEIGHT)
            .build();

        let state = Rc::new(RefCell::new(PreviewState::default()));
        let state_draw = state.clone();
        preview_area.set_draw_func(move |_, cr, w, h| {
            let s = state_draw.borrow();
            if let Some(plan) = s.planned.as_ref() {
                draw_strip(cr, w, h, plan, s.planned_total);
            } else {
                draw_empty(cr, w, h);
            }
        });
        root.append(&preview_area);

        // Legend: one line of colour-coded ■ Name (size) entries.
        let legend = Label::new(None);
        legend.add_css_class("preview-legend");
        legend.set_halign(Align::Start);
        legend.set_wrap(true);
        legend.set_use_markup(true);
        root.append(&legend);

        Self { root, preview_area, legend, state }
    }

    pub fn widget(&self) -> &GtkBox { &self.root }

    pub fn set_disk(&self, disk: Option<Disk>) {
        let mut s = self.state.borrow_mut();
        s.disk = disk;
        s.planned = None;
        s.planned_total = 0;
        drop(s);
        self.preview_area.queue_draw();
        self.update_legend();
    }

    pub fn set_planned(&self, segments: Vec<PlannedSegment>, total: u64) {
        let mut s = self.state.borrow_mut();
        s.planned = Some(segments);
        s.planned_total = total;
        drop(s);
        self.preview_area.queue_draw();
        self.update_legend();
    }

    pub fn clear_planned(&self) {
        let mut s = self.state.borrow_mut();
        s.planned = None;
        s.planned_total = 0;
        drop(s);
        self.preview_area.queue_draw();
        self.update_legend();
    }

    pub fn set_on_reclaim_changed<F: Fn(u64) + 'static>(&self, callback: F) {
        let state_rc = self.state.clone();
        let gesture = GestureDrag::new();
        let cb = Rc::new(callback);
        let cb_drag = cb.clone();

        gesture.connect_drag_update(move |gesture, offset_x, _offset_y| {
            if let Some((start_x, _)) = gesture.start_point() {
                let current_x = start_x + offset_x;

                // Snapshot everything we need and drop the borrow immediately
                // so `cb_drag` can call `set_planned` without a double-borrow.
                let (segs, planned_total) = {
                    let s = state_rc.borrow();
                    match s.planned.as_ref() {
                        Some(segs) if s.planned_total > 0 => (segs.clone(), s.planned_total),
                        _ => return,
                    }
                };

                let widget = gesture.widget().unwrap();
                let w = widget.width() as f64;
                let available = w - 2.0; // 1 px border each side
                let layout = compute_visual(&segs, planned_total, available);

                // Only respond if the drag started near a Resize→NewNimblex handle.
                let mut handle_px_opt: Option<f64> = None;
                for i in 0..segs.len().saturating_sub(1) {
                    if segs[i].action == SegmentAction::Resize
                        && segs[i + 1].action == SegmentAction::NewNimblex
                    {
                        let (lx, lw) = layout[i];
                        handle_px_opt = Some(1.0 + lx + lw);
                        break;
                    }
                }

                match handle_px_opt {
                    Some(hpx) if (start_x - hpx).abs() <= 30.0 => {}
                    _ => return, // drag didn't start near the handle
                }

                // Inverse mapping: convert the current pixel position to a byte
                // position using the actual (non-linear) visual layout.
                let px = (current_x - 1.0).clamp(0.0, available); // strip-relative
                let mut byte_pos = 0u64;
                for (i, &(lx, lw)) in layout.iter().enumerate() {
                    let seg_end = lx + lw;
                    if px <= seg_end || i + 1 == layout.len() {
                        let frac = if lw > 0.0 {
                            ((px - lx) / lw).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        byte_pos = segs[i].start + (frac * segs[i].size as f64) as u64;
                        break;
                    }
                }
                cb_drag(byte_pos);
            }
        });

        self.preview_area.add_controller(gesture);
    }

    fn update_legend(&self) {
        let s = self.state.borrow();
        let markup = match s.planned.as_ref() {
            Some(segs) => build_legend_markup(segs),
            None => match &s.disk {
                Some(d) => format!(
                    "<span foreground=\"#5a6a8a\"><small>{} · {}</small></span>",
                    d.path.display(), d.size
                ),
                None => String::new(),
            },
        };
        self.legend.set_markup(&markup);
    }
}

impl Default for PartitionPreview {
    fn default() -> Self { Self::new() }
}

// ── Legend markup ─────────────────────────────────────────────────────────────

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn build_legend_markup(segs: &[PlannedSegment]) -> String {
    const MIN_LEGEND_BYTES: u64 = 10 * 1024 * 1024; // skip gaps < 10 MiB
    // Small "boring" kept partitions (EFI, MSR, Recovery) clutter the legend;
    // only include them if they're changing or at least 200 MiB.
    const SMALL_KEEP_THRESHOLD: u64 = 200 * 1024 * 1024;
    let mut parts: Vec<String> = Vec::new();
    for s in segs {
        if s.action == SegmentAction::Free && s.size < MIN_LEGEND_BYTES {
            continue;
        }
        let is_system_kept = s.action == SegmentAction::Keep
            && matches!(
                s.role,
                PartitionRole::EfiSystem
                    | PartitionRole::MicrosoftReserved
                    | PartitionRole::WindowsRecovery
            )
            && s.size < SMALL_KEEP_THRESHOLD;
        if is_system_kept {
            continue; // Too small and boring to mention
        }
        let hex = seg_hex_color(s);
        let raw_name = if !s.label.is_empty() {
            s.label.clone()
        } else {
            role_display_name(s.role)
        };
        let name = escape_markup(&raw_name);
        let tag = s.action.legend_tag();
        let size_str = format!("{}", Bytes(s.size));
        parts.push(format!(
            "<span foreground=\"{hex}\">■</span> <b>{name}</b>{tag} <span foreground=\"#5a6a8a\">({size_str})</span>"
        ));
    }
    parts.join("   ")
}

fn role_display_name(role: PartitionRole) -> String {
    match role {
        PartitionRole::EfiSystem => "EFI".into(),
        PartitionRole::MicrosoftReserved => "MSR".into(),
        PartitionRole::WindowsSystem => "Windows".into(),
        PartitionRole::WindowsData => "Windows (data)".into(),
        PartitionRole::WindowsRecovery => "Recovery".into(),
        PartitionRole::Linux => "Linux".into(),
        PartitionRole::LinuxSwap => "Swap".into(),
        PartitionRole::Other => "Other".into(),
    }
}

fn seg_hex_color(s: &PlannedSegment) -> &'static str {
    match s.action {
        SegmentAction::NewNimblex => "#3bd4a3",
        SegmentAction::NewEsp => "#44b3c2",
        SegmentAction::Free => "#2a3450",
        _ => match s.role {
            PartitionRole::WindowsSystem | PartitionRole::WindowsData => "#4a90e2",
            PartitionRole::EfiSystem => "#44b3c2",
            PartitionRole::MicrosoftReserved => "#475569",
            PartitionRole::WindowsRecovery => "#6b7a99",
            PartitionRole::Linux => "#3aa2c2",
            PartitionRole::LinuxSwap => "#f5b800",
            PartitionRole::Other => "#576080",
        },
    }
}

// ── Visual layout (minimum-width two-pass) ────────────────────────────────────

/// Compute `(x_offset, visual_width)` in pixels for each segment.
///
/// Guarantees every segment renders at least `MIN_SEG_PX` wide:
///  1. Assign proportional widths.
///  2. Clamp small segments up to `MIN_SEG_PX`; record the deficit.
///  3. Scale large segments down proportionally to absorb the deficit.
///  4. Accumulate x positions.
fn compute_visual(segs: &[PlannedSegment], total: u64, available: f64) -> Vec<(f64, f64)> {
    if total == 0 || segs.is_empty() {
        return segs.iter().map(|_| (0.0, 0.0)).collect();
    }

    let mut widths: Vec<f64> = segs
        .iter()
        .map(|s| (s.size as f64 / total as f64) * available)
        .collect();

    // One-pass clamp + redistribute.
    let mut deficit = 0.0f64;
    let mut elastic_sum = 0.0f64;
    for &w in &widths {
        if w < MIN_SEG_PX { deficit += MIN_SEG_PX - w; }
        else               { elastic_sum += w; }
    }
    if deficit > 0.0 && elastic_sum > deficit {
        let scale = (elastic_sum - deficit) / elastic_sum;
        for w in &mut widths {
            if *w >= MIN_SEG_PX {
                *w = (*w * scale).max(MIN_SEG_PX);
            } else {
                *w = MIN_SEG_PX;
            }
        }
    }

    // Accumulate x positions.
    let mut result = Vec::with_capacity(segs.len());
    let mut x = 0.0f64;
    for w in widths {
        result.push((x, w));
        x += w;
    }
    result
}

// ── Drawing ────────────────────────────────────────────────────────────────────

fn draw_empty(cr: &Context, w: i32, h: i32) {
    let (w, h) = (w as f64, h as f64);
    rounded_rect(cr, 0.5, 0.5, w - 1.0, h - 1.0, FRAME_RADIUS);
    cr.set_source_rgb(0.060, 0.090, 0.160);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(0.180, 0.235, 0.345);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

fn draw_strip(cr: &Context, w: i32, h: i32, segs: &[PlannedSegment], total: u64) {
    let (w, h) = (w as f64, h as f64);
    if total == 0 { draw_empty(cr, w as i32, h as i32); return; }

    let available = w - 2.0; // 1 px border on each side
    let layout = compute_visual(segs, total, available);

    // Background + clip to rounded rect.
    rounded_rect(cr, 0.5, 0.5, w - 1.0, h - 1.0, FRAME_RADIUS);
    cr.set_source_rgb(0.060, 0.090, 0.160);
    let _ = cr.fill_preserve();
    cr.clip();

    for (i, s) in segs.iter().enumerate() {
        let (lx, segw) = layout[i];
        let px = 1.0 + lx; // offset by 1 px for border

        match s.action {
            SegmentAction::Free => {
                draw_diagonal_hatch(cr, px, 1.0, segw, h - 2.0);
            }
            SegmentAction::NewNimblex => {
                cr.set_source_rgb(0.094, 0.345, 0.275);
                cr.rectangle(px, 1.0, segw, h - 2.0);
                let _ = cr.fill();
                // Brighter "used" portion (left 60%)
                cr.set_source_rgb(0.180, 0.650, 0.490);
                cr.rectangle(px, 1.0, segw * 0.6, h - 2.0);
                let _ = cr.fill();
            }
            _ => {
                let (lo, hi) = role_pair(s.role);
                cr.set_source_rgb(lo.0, lo.1, lo.2);
                cr.rectangle(px, 1.0, segw, h - 2.0);
                let _ = cr.fill();
                // Brighter "used" portion (left 55%)
                cr.set_source_rgb(hi.0, hi.1, hi.2);
                cr.rectangle(px, 1.0, segw * 0.55, h - 2.0);
                let _ = cr.fill();
            }
        }

        // Thin separator between segments (not after the last one).
        if i + 1 < segs.len() {
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.35);
            cr.set_line_width(1.0);
            cr.move_to(px + segw, 1.0);
            cr.line_to(px + segw, h - 1.0);
            let _ = cr.stroke();
        }
    }

    cr.reset_clip();

    // Drag handle between Resize → NewNimblex.
    for i in 0..segs.len().saturating_sub(1) {
        if segs[i].action == SegmentAction::Resize
            && segs[i + 1].action == SegmentAction::NewNimblex
        {
            let (lx, segw) = layout[i];
            draw_drag_handle(cr, 1.0 + lx + segw, h);
        }
    }

    // Border overlay.
    rounded_rect(cr, 0.5, 0.5, w - 1.0, h - 1.0, FRAME_RADIUS);
    cr.set_source_rgb(0.180, 0.235, 0.345);
    cr.set_line_width(1.0);
    let _ = cr.stroke();
}

fn draw_drag_handle(cr: &Context, x: f64, h: f64) {
    let cy = h / 2.0;
    // Drop shadow
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.45);
    rounded_rect(cr, x - 7.0, cy - 15.0, 14.0, 30.0, 7.0);
    let _ = cr.fill();
    // White pill
    cr.set_source_rgb(1.0, 1.0, 1.0);
    rounded_rect(cr, x - 8.0, cy - 16.0, 16.0, 32.0, 8.0);
    let _ = cr.fill();
    // Two grip lines
    cr.set_source_rgb(0.40, 0.40, 0.40);
    cr.set_line_width(1.8);
    cr.move_to(x - 2.5, cy - 7.0); cr.line_to(x - 2.5, cy + 7.0);
    cr.move_to(x + 2.5, cy - 7.0); cr.line_to(x + 2.5, cy + 7.0);
    let _ = cr.stroke();
}

fn draw_diagonal_hatch(cr: &Context, x: f64, y: f64, w: f64, h: f64) {
    cr.save().ok();
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.07);
    cr.set_line_width(1.0);
    let mut t = -h;
    while t < w + h {
        cr.move_to(x + t, y);
        cr.line_to(x + t + h, y + h);
        t += 10.0;
    }
    let _ = cr.stroke();
    cr.restore().ok();
}

/// (dark_fill, bright_used) RGB pair for a partition role.
fn role_pair(role: PartitionRole) -> ((f64, f64, f64), (f64, f64, f64)) {
    match role {
        PartitionRole::WindowsSystem | PartitionRole::WindowsData =>
            ((0.130, 0.220, 0.420), (0.290, 0.560, 0.960)),
        PartitionRole::EfiSystem =>
            ((0.100, 0.280, 0.340), (0.270, 0.700, 0.760)),
        PartitionRole::MicrosoftReserved | PartitionRole::WindowsRecovery =>
            ((0.160, 0.210, 0.310), (0.280, 0.330, 0.430)),
        PartitionRole::Linux =>
            ((0.090, 0.300, 0.300), (0.227, 0.637, 0.760)),
        PartitionRole::LinuxSwap =>
            ((0.480, 0.380, 0.120), (0.960, 0.760, 0.320)),
        PartitionRole::Other =>
            ((0.180, 0.210, 0.290), (0.330, 0.380, 0.480)),
    }
}

fn rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let pi = std::f64::consts::PI;
    cr.new_sub_path();
    cr.arc(x + w - r, y + r,     r, -pi / 2.0,  0.0);
    cr.arc(x + w - r, y + h - r, r,  0.0,        pi / 2.0);
    cr.arc(x + r,     y + h - r, r,  pi / 2.0,   pi);
    cr.arc(x + r,     y + r,     r,  pi,          1.5 * pi);
    cr.close_path();
}
