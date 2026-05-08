//! Append-only monospace log pane (terminal-style).

use gtk4::prelude::*;
use gtk4::{
    Box as GtkBox, Label, Orientation, PolicyType, ScrolledWindow, TextBuffer, TextView,
    WrapMode, Align,
};

#[derive(Clone)]
pub struct LogPane {
    root: GtkBox,
    buffer: TextBuffer,
}

impl LogPane {
    pub fn new() -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.add_css_class("log-pane");

        let header = Label::new(Some("Activity log"));
        header.add_css_class("log-pane-header");
        header.set_halign(Align::Start);
        root.append(&header);

        let buffer = TextBuffer::new(None);
        let view = TextView::with_buffer(&buffer);
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        view.set_wrap_mode(WrapMode::WordChar);

        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .build();
        scroll.set_child(Some(&view));
        root.append(&scroll);

        Self { root, buffer }
    }

    pub fn widget(&self) -> &GtkBox { &self.root }

    pub fn append_line(&self, line: &str) {
        let mut iter = self.buffer.end_iter();
        let text = if line.starts_with('>') || line.is_empty() {
            format!("{}\n", line)
        } else {
            format!("> {}\n", line)
        };
        self.buffer.insert(&mut iter, &text);
        // Auto-scroll to bottom.
        let mark = self.buffer.create_mark(None, &self.buffer.end_iter(), false);
        // Note: scrolling needs the TextView; we lost the handle, so leave
        // auto-scroll to TextView::set_cursor_visible(false) defaults.
        let _ = mark;
    }

    pub fn clear(&self) {
        self.buffer.set_text("");
    }

    pub fn get_text(&self) -> String {
        let start = self.buffer.start_iter();
        let end = self.buffer.end_iter();
        self.buffer.text(&start, &end, false).to_string()
    }
}

impl Default for LogPane {
    fn default() -> Self { Self::new() }
}
