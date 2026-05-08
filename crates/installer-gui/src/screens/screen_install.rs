//! Screen 2 — Install.
//!
//! Big circular progress + log pane. Spawns the helper (with `pkexec`
//! when needed) and parses its line-delimited JSON event stream into
//! progress + log updates.

use crate::state::AppState;
use crate::widgets::{app_card, CircularProgress, Header, HeaderStep, LogPane};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Align, Box as GtkBox, Button, Label, Orientation, Stack, Window};
use serde::Deserialize;
use std::cell::{Cell, RefCell};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

/// Returns `true` for lines that are technically expected but alarming or
/// meaningless to end users, so we suppress them from the visible log pane.
fn is_noise_line(line: &str) -> bool {
    const NOISE: &[&str] = &[
        // sgdisk warnings on live-boot USB (expected, harmless)
        "Warning: The kernel is still using the old partition table",
        "The new table will be used at the next reboot",
        "run partprobe(8) or kpartx(8)",
        "GPT data structures destroyed",
        // udev config deprecation from old udev.conf keys
        "/etc/udev/udev.conf:",
        // mke2fs informational flood
        "Superblock backups stored on blocks:",
        // mkfs.ext4 hint when -O 64bit is missing (fixed in planner, guard for transitions)
        "64-bit filesystem support is not enabled",
        "Pass -O 64bit to rectify",
    ];
    NOISE.iter().any(|pat| line.contains(pat))
}

/// Mirror of `installer_helper::events::Event` — kept in sync by JSON shape.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HelperEvent {
    PlanAccepted { steps: usize },
    StepStart { index: usize, label: String },
    Stdout { index: usize, line: String },
    Stderr { index: usize, line: String },
    StepDone { index: usize, exit_code: i32 },
    Complete,
    Error { index: Option<usize>, message: String },
}

#[derive(Clone)]
pub struct ScreenInstall {
    root: GtkBox,
    state: Rc<RefCell<AppState>>,
    progress: CircularProgress,
    log: LogPane,
    title: Label,
    subtitle: Label,
    finish_btn: Button,
    back_btn: Button,
    copy_log_btn: Button,
    started: Rc<Cell<bool>>,
}

impl ScreenInstall {
    pub fn new(stack: Stack, state: Rc<RefCell<AppState>>) -> Self {
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        let card = app_card();
        let header = Header::new(HeaderStep::Install);
        card.append(header.widget());

        let stage = GtkBox::new(Orientation::Vertical, 14);
        stage.add_css_class("install-stage");

        let progress = CircularProgress::new();
        // Caption left blank initially; populated when the first
        // step starts. It is rendered BELOW the circle so it can never
        // overflow the ring.
        progress.set_caption("");
        stage.append(progress.widget());

        let title = Label::new(Some("Installing Nimblex"));
        title.add_css_class("screen-h1");
        title.set_halign(Align::Center);
        stage.append(&title);

        let subtitle = Label::new(Some("Please don't unplug or power off."));
        subtitle.add_css_class("screen-subtitle");
        subtitle.set_halign(Align::Center);
        stage.append(&subtitle);

        let log = LogPane::new();
        stage.append(log.widget());

        card.append(&stage);

        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.add_css_class("app-footer");
        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        
        let back_btn = Button::with_label("Back");
        back_btn.add_css_class("btn-secondary");
        back_btn.set_visible(false);
        footer.append(&back_btn);
        
        let copy_log_btn = Button::with_label("Copy Log");
        copy_log_btn.add_css_class("btn-secondary");
        copy_log_btn.set_visible(false);
        footer.append(&copy_log_btn);

        let finish_btn = Button::with_label("Close");
        finish_btn.add_css_class("btn-primary");
        finish_btn.set_sensitive(false);
        footer.append(&finish_btn);
        card.append(&footer);

        let finish_close = finish_btn.clone();
        finish_btn.connect_clicked(move |_| {
            if let Some(win) = finish_close.root().and_downcast::<Window>() {
                win.close();
            }
        });
        
        let stack_back = stack.clone();
        let back_btn_clone = back_btn.clone();
        let copy_log_btn_clone = copy_log_btn.clone();
        
        let started_clone = Rc::new(Cell::new(false));
        let started_back = started_clone.clone();
        let progress_back = progress.clone();
        let log_back = log.clone();
        let title_back = title.clone();
        let subtitle_back = subtitle.clone();
        let finish_back = finish_btn.clone();
        let back_btn_back = back_btn.clone();
        let copy_log_back = copy_log_btn.clone();

        back_btn.connect_clicked(move |_| {
            // Reset state
            started_back.set(false);
            progress_back.reset();
            log_back.clear();
            title_back.set_text("Installing Nimblex");
            title_back.remove_css_class("title-error");
            title_back.remove_css_class("title-success");
            subtitle_back.set_text("");
            finish_back.set_label("Close");
            finish_back.set_sensitive(false);
            back_btn_back.set_visible(false);
            copy_log_back.set_visible(false);
            
            // Go back to destination screen
            stack_back.set_visible_child_name(crate::app::STACK_DEST);
        });

        let log_clone = log.clone();
        copy_log_btn.connect_clicked(move |btn| {
            let log_text = log_clone.get_text();
            let clipboard = btn.clipboard();
            clipboard.set_text(&log_text);
            btn.set_label("Copied!");
            glib::timeout_add_local(std::time::Duration::from_secs(2), {
                let b = btn.clone();
                move || {
                    b.set_label("Copy Log");
                    glib::ControlFlow::Break
                }
            });
        });

        root.append(&card);

        Self {
            root,
            state,
            progress,
            log,
            title,
            subtitle,
            finish_btn,
            back_btn: back_btn_clone,
            copy_log_btn: copy_log_btn_clone,
            started: started_clone,
        }
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    /// Called when the screen becomes visible. Starts the helper exactly once.
    pub fn start(&self) {
        if self.started.get() {
            return;
        }
        self.started.set(true);

        let (plan_json, weights, total_steps) = match self.state.borrow().plan.as_ref() {
            Some(p) => {
                let weights: Vec<f64> = p.steps.iter().map(|s| s.weight).collect();
                let total_steps = p.steps.len();
                match serde_json::to_string(p) {
                    Ok(j) => (j, weights, total_steps),
                    Err(e) => {
                        self.log.append_line(&format!("encode plan failed: {}", e));
                        return;
                    }
                }
            }
            None => {
                self.log.append_line("no plan in state");
                return;
            }
        };

        // Build cumulative weight table.
        // cumulative[i] = fraction of bar completed just before step i starts.
        let total_weight = weights.iter().sum::<f64>().max(1.0);
        let mut cumulative = vec![0.0f64; weights.len() + 1];
        for i in 0..weights.len() {
            cumulative[i + 1] = cumulative[i] + weights[i] / total_weight;
        }
        let cumulative = Rc::new(cumulative);
        let weights = Rc::new(weights);

        // Worker thread sends WorkerMsg back; main thread (timeout) drains.
        let (tx, rx) = mpsc::channel::<WorkerMsg>();
        let tx_worker = tx.clone();
        std::thread::spawn(move || {
            run_helper(&plan_json, tx_worker);
        });
        // drop our extra sender clone so the channel closes when worker exits.
        drop(tx);

        let me = self.clone();
        let rx = Rc::new(RefCell::new(rx));
        // Track which step is current so PROGRESS: lines can interpolate.
        let cur_step_base = Rc::new(Cell::new(0.0f64));
        let cur_step_span = Rc::new(Cell::new(0.0f64));
        glib::timeout_add_local(Duration::from_millis(40), move || {
            let r = rx.borrow();
            let mut closed = false;
            loop {
                match r.try_recv() {
                    Ok(WorkerMsg::Event(ev)) => me.handle_event(
                        ev,
                        total_steps,
                        &cumulative,
                        &weights,
                        &cur_step_base,
                        &cur_step_span,
                    ),
                    Ok(WorkerMsg::RawLine(line)) => me.log.append_line(&line),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        closed = true;
                        break;
                    }
                }
            }
            if closed {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn handle_event(
        &self,
        event: HelperEvent,
        total_steps: usize,
        cumulative: &[f64],
        weights: &[f64],
        cur_step_base: &Cell<f64>,
        cur_step_span: &Cell<f64>,
    ) {
        let _ = (total_steps, weights); // may be used for labels
        match event {
            HelperEvent::PlanAccepted { steps } => {
                self.log
                    .append_line(&format!("plan accepted: {} steps", steps));
            }
            HelperEvent::StepStart { index, label } => {
                let base = cumulative.get(index).copied().unwrap_or(0.0);
                let span = cumulative.get(index + 1).copied().unwrap_or(1.0) - base;
                cur_step_base.set(base);
                cur_step_span.set(span);
                self.progress.set_progress(base);
                self.progress.set_caption(&label);
                self.log.append_line(&format!(
                    "[{}/{}] {}",
                    index + 1,
                    cumulative.len().saturating_sub(1),
                    label
                ));
            }
            HelperEvent::Stdout { line, .. } => {
                // Parse PROGRESS:NN emitted by copy-system; interpolate within
                // the current step's weight range.  Don't show these raw lines
                // in the log — they'd flood it.
                if let Some(rest) = line.strip_prefix("PROGRESS:") {
                    if let Ok(pct) = rest.trim().parse::<u32>() {
                        let sub = (pct as f64 / 100.0).clamp(0.0, 1.0);
                        self.progress.set_progress(
                            cur_step_base.get() + sub * cur_step_span.get(),
                        );
                    }
                } else if !is_noise_line(&line) {
                    self.log.append_line(&line);
                }
            }
            HelperEvent::Stderr { line, .. } => {
                if !is_noise_line(&line) {
                    self.log.append_line(&line);
                }
            }
            HelperEvent::StepDone { index, .. } => {
                let done = cumulative.get(index + 1).copied().unwrap_or(1.0);
                self.progress.set_progress(done);
            }
            HelperEvent::Complete => {
                self.progress.set_progress(1.0);
                self.progress.set_caption("Done");
                self.title.set_text("Install complete");
                self.title.add_css_class("title-success");
                self.subtitle.set_text("Reboot to start Nimblex.");
                self.finish_btn.set_sensitive(true);
                self.copy_log_btn.set_visible(true);
            }
            HelperEvent::Error { message, .. } => {
                self.progress.set_failed();
                self.title.set_text("Install failed");
                self.title.add_css_class("title-error");
                self.subtitle.set_text("See the log below for details.");
                self.log.append_line(&format!("ERROR: {}", message));
                self.finish_btn.set_sensitive(true);
                self.back_btn.set_visible(true);
                self.copy_log_btn.set_visible(true);
            }
        }
    }
}

fn helper_path() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("nimblex-installer-helper");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from("/usr/libexec/nimblex-installer-helper")
}

fn nix_geteuid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}

enum WorkerMsg {
    Event(HelperEvent),
    RawLine(String),
}

fn run_helper(plan_json: &str, tx: mpsc::Sender<WorkerMsg>) {
    let helper = helper_path();
    let is_root = nix_geteuid() == 0;
    let mut cmd = if is_root {
        Command::new(&helper)
    } else {
        let mut c = Command::new("pkexec");
        c.arg(&helper);
        c
    };
    cmd.arg("--run");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(WorkerMsg::Event(HelperEvent::Error {
                index: None,
                message: format!("failed to spawn helper: {}", e),
            }));
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(plan_json.as_bytes());
    }

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            match serde_json::from_str::<HelperEvent>(&line) {
                Ok(ev) => {
                    let _ = tx.send(WorkerMsg::Event(ev));
                }
                Err(_) => {
                    let _ = tx.send(WorkerMsg::RawLine(line));
                }
            }
        }
    }

    let _ = child.wait();
}
