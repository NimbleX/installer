//! Typed install plan.
//!
//! A [`Plan`] is an ordered list of [`Step`]s. Each step carries the exact
//! shell argv that will be executed; the helper validates argv shapes against
//! an allowlist before running them. Plans are serialised to JSON over stdin
//! when the GUI invokes the helper.

use crate::scenario::Scenario;
use crate::size::Bytes;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub scenario: Scenario,
    /// Disk device the plan operates on, e.g. `/dev/sdb` for USB or
    /// `/dev/nvme0n1` for an internal install.
    pub target_disk: PathBuf,
    /// For `AlongsideWindows`: the NTFS partition to be shrunk.
    pub shrink_partition: Option<PathBuf>,
    /// For `AlongsideWindows`: target size of the Windows partition after shrink.
    pub shrink_to: Option<Bytes>,
    /// For `AlongsideWindows`: path the new Nimblex root partition will receive.
    pub new_root_partition: Option<PathBuf>,
    /// Size of the Nimblex root partition we are about to create.
    pub new_root_size: Bytes,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// Short, user-facing label rendered next to the pictograph on Screen 3.
    pub label: String,
    pub category: StepCategory,
    /// argv (program + args) to execute. Pipelines are expressed as multiple
    /// steps; the helper does not invoke a shell.
    pub argv: Vec<String>,
    /// Whether a failure of this step should stop the whole plan.
    /// Always `true` for v1; reserved for future advisory steps.
    pub critical: bool,
    /// Whether this step modifies on-disk data. Used for the "destructive
    /// actions counter" the GUI shows on the confirm screen.
    pub destructive: bool,
    /// Relative weight for progress bar computation. `copy-system` gets 70.0
    /// while fast partition/format steps get 0.5–3.0. The GUI normalises the
    /// sum to 1.0 so only the ratio between steps matters.
    pub weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepCategory {
    /// Read-only probe (`lsblk`, `ntfsresize --info`, `f3probe`).
    Probe,
    /// Disable Windows Fast Startup via chntpw.
    PrepareWindows,
    /// `ntfsresize` shrink + `parted resizepart`.
    Resize,
    /// `parted mkpart` / `sgdisk` create new partitions.
    Partition,
    /// `mkfs.*`.
    Format,
    /// `cp`/`rsync`/`pv` of bundles, kernel, initrd into place.
    Copy,
    /// Install bootloader / register EFI entry.
    Bootloader,
    /// `sync`, unmount, finalise.
    Finalise,
}

impl StepCategory {
    /// One-word icon name (matches `assets/icons/<name>.svg`).
    pub fn icon(&self) -> &'static str {
        match self {
            StepCategory::Probe => "probe",
            StepCategory::PrepareWindows => "windows-prepare",
            StepCategory::Resize => "shrink",
            StepCategory::Partition => "partition",
            StepCategory::Format => "format",
            StepCategory::Copy => "copy",
            StepCategory::Bootloader => "boot",
            StepCategory::Finalise => "check",
        }
    }

    /// Human-readable section heading for the "Show commands" transcript.
    pub fn section(&self) -> &'static str {
        match self {
            StepCategory::Probe => "Inspect the disk (read-only)",
            StepCategory::PrepareWindows => "Prepare Windows for safe resize",
            StepCategory::Resize => "Shrink the Windows partition",
            StepCategory::Partition => "Create the Nimblex partition",
            StepCategory::Format => "Format the new partition",
            StepCategory::Copy => "Copy Nimblex onto the disk",
            StepCategory::Bootloader => "Install the bootloader",
            StepCategory::Finalise => "Finalise and clean up",
        }
    }
}

impl Plan {
    /// One-sentence English summary suitable for the confirmation overlay.
    /// Examples:
    ///   "Erase /dev/sdb and install Nimblex (15.5 GiB)."
    ///   "Shrink Windows to 348 GiB and install Nimblex (163 GiB)."
    pub fn summary_one_line(&self) -> String {
        match self.scenario {
            Scenario::UsbFullInstall => format!(
                "Erase {} and install Nimblex ({}).",
                self.target_disk.display(),
                self.new_root_size
            ),
            Scenario::AlongsideWindows => format!(
                "Shrink Windows to {} and install Nimblex ({}).",
                self.shrink_to.unwrap_or(Bytes(0)),
                self.new_root_size
            ),
        }
    }

    /// Render the plan as a human-readable shell transcript for the
    /// "Show commands" modal. Steps are grouped by category with bold
    /// section headers and a one-line explanation; commands are shell-quoted
    /// so they can be copy-pasted. Output is byte-stable for snapshot tests.
    pub fn shell_transcript(&self) -> String {
        let mut out = String::new();
        // ---- Header ----
        out.push_str("══════════════════════════════════════════════════════\n");
        out.push_str(&format!(
            " Nimblex installer — {}\n",
            scenario_label(&self.scenario)
        ));
        out.push_str("══════════════════════════════════════════════════════\n");
        out.push_str(&format!("Target disk:        {}\n", self.target_disk.display()));
        if let Some(p) = &self.shrink_partition {
            out.push_str(&format!(
                "Shrink:             {}  →  {}\n",
                p.display(),
                self.shrink_to.unwrap_or(Bytes(0))
            ));
        }
        if let Some(p) = &self.new_root_partition {
            out.push_str(&format!(
                "New Nimblex root:   {}  ({})\n",
                p.display(),
                self.new_root_size
            ));
        }
        let destructive = self.steps.iter().filter(|s| s.destructive).count();
        out.push_str(&format!(
            "Steps:              {} total, {} destructive\n",
            self.steps.len(),
            destructive
        ));
        out.push('\n');

        // ---- Step groups ----
        let mut current_section: Option<StepCategory> = None;
        for (i, s) in self.steps.iter().enumerate() {
            if current_section != Some(s.category) {
                if current_section.is_some() {
                    out.push('\n');
                }
                out.push_str(&format!("── {} ──\n", s.category.section()));
                current_section = Some(s.category);
            }
            let marker = if s.destructive { "!" } else { " " };
            out.push_str(&format!(
                "\n  [{:>2}/{}] {} {}\n",
                i + 1,
                self.steps.len(),
                marker,
                s.label
            ));
            out.push_str("        $ ");
            out.push_str(&shell_quote(&s.argv));
            out.push('\n');
        }
        out.push('\n');
        out.push_str("Legend: ! = modifies on-disk data\n");
        out
    }
}

fn scenario_label(s: &Scenario) -> &'static str {
    match s {
        Scenario::UsbFullInstall => "Install on USB stick (whole disk)",
        Scenario::AlongsideWindows => "Install alongside Windows",
    }
}

/// Quote argv into a copy-pasteable shell command. We never *execute* via a
/// shell; this is purely for human display.
fn shell_quote(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty()
                || a.chars().any(|c| {
                    !(c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '=' | ':'))
                })
            {
                let escaped = a.replace('\'', "'\\''");
                format!("'{}'", escaped)
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_quotes_special_chars() {
        let p = Plan {
            scenario: Scenario::UsbFullInstall,
            target_disk: PathBuf::from("/dev/sdb"),
            shrink_partition: None,
            shrink_to: None,
            new_root_partition: None,
            new_root_size: Bytes::from_gib(8),
            steps: vec![Step {
                label: "label".into(),
                category: StepCategory::Format,
                argv: vec![
                    "mkfs.ext4".into(),
                    "-L".into(),
                    "Nimblex Root".into(),
                    "/dev/sdb2".into(),
                ],
                critical: true,
                destructive: true,
                weight: 1.0,
            }],
        };
        let t = p.shell_transcript();
        assert!(t.contains("'Nimblex Root'"));
        assert!(t.contains("/dev/sdb2"));
    }
}
