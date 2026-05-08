//! Big circular progress widget used by the Install screen.
//!
//! Cairo-drawn arc + big % label inside the ring. The step caption is
//! exposed as a separate label rendered **below** the ring (so long
//! captions never overflow). Supports a `failed` state that recolours
//! the ring red and replaces the percent number with `✕`.

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, DrawingArea, Label, Image, Orientation};
use std::cell::Cell;
use std::rc::Rc;

const SIZE: i32 = 220;
const STROKE: f64 = 14.0;

#[derive(Clone)]
pub struct CircularProgress {
    root: GtkBox,
    area: DrawingArea,
    percent_label: Label,
    error_icon: Image,
    caption_label: Label,
    progress: Rc<Cell<f64>>,
    failed: Rc<Cell<bool>>,
}

impl CircularProgress {
    pub fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 20);
        root.set_halign(Align::Center);

        let area = DrawingArea::builder()
            .width_request(SIZE)
            .height_request(SIZE)
            .content_width(SIZE)
            .content_height(SIZE)
            .build();

        let progress: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
        let failed: Rc<Cell<bool>> = Rc::new(Cell::new(false));
        let prog_draw = progress.clone();
        let failed_draw = failed.clone();
        area.set_draw_func(move |_a, cr, w, h| {
            draw(cr, w as f64, h as f64, prog_draw.get(), failed_draw.get());
        });

        // Only the percent number lives inside the circle.
        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&area));

        let percent_label = Label::new(Some("0%"));
        percent_label.add_css_class("progress-percent");
        percent_label.set_halign(Align::Center);
        percent_label.set_valign(Align::Center);
        overlay.add_overlay(&percent_label);

        let error_icon = Image::from_icon_name("dialog-error-symbolic");
        error_icon.set_pixel_size(100);
        error_icon.add_css_class("progress-error-icon");
        error_icon.set_halign(Align::Center);
        error_icon.set_valign(Align::Center);
        error_icon.set_visible(false);
        overlay.add_overlay(&error_icon);
        root.append(&overlay);

        // Caption sits below the circle; safe for long text. Hidden until
        // the first non-empty caption is set, so the install screen at 0%
        // doesn't display an empty placeholder line.
        let caption_label = Label::new(Some(""));
        caption_label.add_css_class("progress-caption");
        caption_label.set_halign(Align::Center);
        caption_label.set_wrap(true);
        caption_label.set_max_width_chars(48);
        caption_label.set_visible(false);
        root.append(&caption_label);

        Self {
            root,
            area,
            percent_label,
            error_icon,
            caption_label,
            progress,
            failed,
        }
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    pub fn set_progress(&self, fraction: f64) {
        let f = fraction.clamp(0.0, 1.0);
        self.progress.set(f);
        if !self.failed.get() {
            self.percent_label
                .set_text(&format!("{}%", (f * 100.0).round() as i32));
        }
        self.area.queue_draw();
    }

    pub fn set_caption(&self, caption: &str) {
        self.caption_label.set_text(caption);
        self.caption_label.set_visible(!caption.is_empty());
    }

    /// Switch to the failed state: red ring, large `✕` instead of percent,
    /// caption hidden. Idempotent.
    pub fn set_failed(&self) {
        self.failed.set(true);
        self.percent_label.set_visible(false);
        self.error_icon.set_visible(true);
        self.caption_label.set_visible(false);
        self.area.queue_draw();
    }
    
    pub fn reset(&self) {
        self.failed.set(false);
        self.progress.set(0.0);
        self.percent_label.set_text("0%");
        self.percent_label.set_visible(true);
        self.error_icon.set_visible(false);
        self.caption_label.set_visible(false);
        self.area.set_visible(true);
        self.area.queue_draw();
    }
}

impl Default for CircularProgress {
    fn default() -> Self {
        Self::new()
    }
}

fn draw(cr: &Context, w: f64, h: f64, fraction: f64, failed: bool) {
    if failed {
        // Do not draw the circle track at all on failure, just leave the space
        // so the warning icon can sit cleanly in the center.
        return;
    }

    let cx = w / 2.0;
    let cy = h / 2.0;
    let radius = (w.min(h) / 2.0) - STROKE - 2.0;

    // Track.
    cr.set_source_rgb(0.082, 0.125, 0.220); // dark navy
    cr.set_line_width(STROKE);
    cr.arc(cx, cy, radius, 0.0, std::f64::consts::PI * 2.0);
    let _ = cr.stroke();

    // Progress arc
    let (frac, r, g, b) = (fraction, 0.231, 0.831, 0.639); // teal-mint
    if frac > 0.0 {
        let start = -std::f64::consts::PI / 2.0;
        let end = start + frac * std::f64::consts::PI * 2.0;
        cr.set_source_rgb(r, g, b);
        cr.set_line_width(STROKE);
        cr.set_line_cap(gtk4::cairo::LineCap::Round);
        cr.arc(cx, cy, radius, start, end);
        let _ = cr.stroke();
    }
}
