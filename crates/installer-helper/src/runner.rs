//! Step execution loop and dry-run printer.

use crate::allowlist;
use crate::events::Event;
use anyhow::Result;
use installer_core::Plan;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

/// Validate the plan and print every step's argv. No side effects.
pub fn dry_run(plan: &Plan) -> Result<()> {
    allowlist::validate_steps(&plan.steps)?;
    Event::PlanAccepted {
        steps: plan.steps.len(),
    }
    .emit();
    for (i, step) in plan.steps.iter().enumerate() {
        Event::StepStart {
            index: i,
            label: step.label.clone(),
        }
        .emit();
        Event::Stdout {
            index: i,
            line: format!("(dry-run) {}", shell_join(&step.argv)),
        }
        .emit();
        Event::StepDone {
            index: i,
            exit_code: 0,
        }
        .emit();
    }
    Event::Complete.emit();
    Ok(())
}

/// Validate then execute every step in order. Streams subprocess output as
/// JSON events. Aborts at the first failing step.
pub fn execute(plan: &Plan) -> Result<()> {
    allowlist::validate_steps(&plan.steps)?;
    Event::PlanAccepted {
        steps: plan.steps.len(),
    }
    .emit();

    for (i, step) in plan.steps.iter().enumerate() {
        Event::StepStart {
            index: i,
            label: step.label.clone(),
        }
        .emit();

        let exit = run_step(i, step.argv.as_slice());
        match exit {
            Ok(code) if code == 0 => {
                Event::StepDone {
                    index: i,
                    exit_code: 0,
                }
                .emit();
            }
            Ok(code) => {
                let msg = format!("step {} exited with code {}", i + 1, code);
                Event::Error {
                    index: Some(i),
                    message: msg.clone(),
                }
                .emit();
                anyhow::bail!(msg);
            }
            Err(e) => {
                let msg = format!("step {} failed to spawn: {}", i + 1, e);
                Event::Error {
                    index: Some(i),
                    message: msg.clone(),
                }
                .emit();
                anyhow::bail!(msg);
            }
        }
    }

    Event::Complete.emit();
    Ok(())
}

fn run_step(index: usize, argv: &[String]) -> Result<i32> {
    // The planner emits internal-helper steps with argv[0] =
    // "nimblex-installer-helper-internal". This is an argv0-alias that
    // resolves to the helper binary itself (see main.rs dispatch).
    // PATH lookup of that name fails on most systems, so redirect to
    // the currently-running executable while preserving the argv0
    // alias so the helper's argv0 dispatch still triggers internal mode.
    const INTERNAL_ALIAS: &str = "nimblex-installer-helper-internal";
    let mut cmd = if argv[0] == INTERNAL_ALIAS {
        let exe = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from(&argv[0]));
        let mut c = Command::new(exe);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            c.arg0(INTERNAL_ALIAS);
        }
        c
    } else {
        Command::new(&argv[0])
    };
    cmd.args(&argv[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn()?;

    // Drain stdout and stderr concurrently so a chatty tool can't deadlock
    // on a full pipe buffer.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_thread = stdout.map(|s| {
        std::thread::spawn(move || {
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                Event::Stdout {
                    index,
                    line,
                }
                .emit();
            }
        })
    });
    let stderr_thread = stderr.map(|s| {
        std::thread::spawn(move || {
            for line in BufReader::new(s).lines().map_while(Result::ok) {
                Event::Stderr {
                    index,
                    line,
                }
                .emit();
            }
        })
    });

    let status = child.wait()?;
    if let Some(t) = stdout_thread {
        let _ = t.join();
    }
    if let Some(t) = stderr_thread {
        let _ = t.join();
    }
    Ok(status.code().unwrap_or(-1))
}

fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() || a.contains(char::is_whitespace) {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
