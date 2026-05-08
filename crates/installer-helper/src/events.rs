//! JSON event protocol between the helper and the GUI.
//!
//! Each event is a single line of JSON written to stdout. The GUI reads
//! line-by-line and feeds events into its progress UI.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// Plan-level: helper has parsed and validated the plan.
    PlanAccepted { steps: usize },
    /// A step is about to start.
    StepStart { index: usize, label: String },
    /// One line of subprocess stdout.
    Stdout { index: usize, line: String },
    /// One line of subprocess stderr.
    Stderr { index: usize, line: String },
    /// Step finished successfully.
    StepDone { index: usize, exit_code: i32 },
    /// Plan finished successfully.
    Complete,
    /// Hard error; the plan is aborted.
    Error { index: Option<usize>, message: String },
}

impl Event {
    /// Serialise to a single line (no trailing newline).
    pub fn to_line(&self) -> String {
        // Should never fail for our types; if it does, surface a stub event
        // so callers don't have to handle Result on every emit.
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!(r#"{{"kind":"error","message":"event encode failed: {}"}}"#, e)
        })
    }

    pub fn emit(&self) {
        println!("{}", self.to_line());
    }
}
