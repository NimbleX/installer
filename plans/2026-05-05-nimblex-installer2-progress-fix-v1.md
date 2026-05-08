# Nimblex Installer — Progress Bar Accuracy Fix

## Objective

Fix the progress bar so it reflects actual elapsed work rather than step count.
Currently 8 fast partition/format steps fill 72% of the bar in seconds, and the
slow `copy-system` step (copies 5–8 GB) gets only one step's worth (~9%) with no
sub-step updates. After this fix:
- `copy-system` owns ~72% of the bar
- The bar advances in real-time as each bundle is copied (every 5 MB)
- All other steps share the remaining ~28% proportionally

## Implementation Plan

### Part 1 — Add `weight` field to `Step` (`crates/installer-core/src/plan.rs`)

- [ ] Task 1. Add `pub weight: f64` field to the `Step` struct, directly after the
  `destructive: bool` field.  
  Add a doc comment: `/// Relative weight for progress (e.g. copy-system = 70.0,
  partition steps = 1.0). GUI normalises all weights to [0, 1].`

### Part 2 — Assign weights in the planner (`crates/installer-core/src/planner.rs`)

All existing `Step { … }` literals need a `weight:` field appended. Use these values:

**USB plan (`plan_usb`)**:
| Step label | `weight` |
|---|---|
| Erase old partition table | `1.0` |
| Create GPT partition table | `0.5` |
| Create EFI System Partition | `0.5` |
| Create Nimblex root partition | `0.5` |
| Reload partition table | `0.5` |
| Format ESP as FAT32 | `2.0` |
| Format Nimblex root as ext4 | `3.0` |
| **Copy Nimblex bundles** | **`70.0`** |
| Install bootloader (UEFI + BIOS) | `8.0` |
| Flush and finalise | `2.0` |

- [ ] Task 2. Add `weight` fields to all USB plan steps per the table above.

**Alongside Windows plan (`plan_alongside`)**:
| Step label | `weight` |
|---|---|
| Disable Windows Fast Startup | `2.0` |
| Measure minimum resize size (`ntfsresize --info`) | `1.0` |
| Shrink NTFS filesystem | `6.0` |
| Resize partition in table | `1.0` |
| Create Nimblex partition | `1.0` |
| Reload partition table | `0.5` |
| Format Nimblex root as ext4 | `3.0` |
| **Copy Nimblex bundles** | **`60.0`** |
| Install bootloader | `8.0` |
| Flush and finalise | `2.0` |

- [ ] Task 3. Add `weight` fields to all alongside-Windows plan steps per the table above.

### Part 3 — Improve copy progress in the helper (`crates/installer-helper/src/internal.rs`)

The current `copy_system_inner` emits per-file percentages (`    10%`, `    20%`, …).
This is hard to interpret because there are multiple bundles and the numbers reset to 0 each time.

- [ ] Task 4. In `copy_system_inner`, before the copy loop starts, compute
  `total_bytes_all: u64` = sum of `entry.metadata().map(|m| m.len()).unwrap_or(0)`
  for every bundle entry.

- [ ] Task 5. Remove the `copy_file_with_progress` function entirely. Replace calls
  to it with a direct read+write loop inside `copy_system_inner` that increments a
  `total_copied: u64` counter after each chunk. After each chunk write, if
  `total_bytes_all > 0`, compute `pct = (total_copied * 100 / total_bytes_all) as u32`
  and, when it has advanced by ≥ 2 since `last_pct`, print the line:
  ```
  PROGRESS:NN
  ```
  where `NN` is the integer percentage. Use 4 MiB chunks (same as now).

- [ ] Task 6. Also emit `PROGRESS:0` immediately before the first bundle's first
  chunk is read, so the GUI knows copying has started.

### Part 4 — Weighted progress in the GUI (`crates/installer-gui/src/screens/screen_install.rs`)

- [ ] Task 7. In `ScreenInstall`, add two new `Rc<Cell<f64>>` fields:
  - `step_base: Rc<Cell<f64>>` — progress at the start of the current step
  - `step_span: Rc<Cell<f64>>` — fraction of total bar owned by the current step

- [ ] Task 8. Compute `total_weight: f64` from the plan in state when `start()` is
  called. Store it as a local variable (not a field — it does not change). Also
  pre-compute a `Vec<f64>` of cumulative weights:
  `cumulative[i] = sum of weight[0..i] / total_weight`.

  Example: 10 steps with weights `[1, 0.5, 0.5, 0.5, 0.5, 2, 3, 70, 8, 2]`
  → `total = 88`, `cumulative[7] = (1+0.5+0.5+0.5+0.5+2+3) / 88 = 0.0909`.
  
  Make `cumulative` available inside the event closure by wrapping it in
  `Rc<Vec<f64>>`.

- [ ] Task 9. Update the `StepStart { index, label }` arm of `handle_event`:
  - Set `step_base` = `cumulative[index]`
  - Set `step_span` = `weights[index] / total_weight`
  - Call `self.progress.set_progress(step_base)`

- [ ] Task 10. Update the `Stdout { line, .. }` arm of `handle_event`:
  - If `line.starts_with("PROGRESS:")` and the suffix parses as a `u32`:
    - `sub_pct = parsed_value as f64 / 100.0` (clamped to `[0.0, 1.0]`)
    - Call `self.progress.set_progress(step_base + sub_pct * step_span)`
  - Otherwise: forward to log as before.
  - Also do not add `PROGRESS:XX` lines to the log pane (skip `log.append_line`
    for lines that start with `"PROGRESS:"`).

- [ ] Task 11. Update the `StepDone { index, .. }` arm:
  - `frac = cumulative[index] + weights[index] / total_weight`
  - Call `self.progress.set_progress(frac)`

### Part 5 — Update unit tests (`crates/installer-core/src/planner.rs`)

- [ ] Task 12. The existing `usb_plan_has_expected_step_categories` test calls
  `InstallPlanner::plan_usb(…)`. Since `Step` now requires a `weight` field, the
  test will fail to compile if any `Step` literal is missing it. Run
  `cargo check -p installer-core` to verify, and fix any struct-literal errors.

- [ ] Task 13. The `transcript_quotes_special_chars` test in `plan.rs` constructs
  a `Step` directly. Add `weight: 1.0` to that literal.

### Part 6 — Build, verify, and relaunch

- [ ] Task 14. Run `cargo build --release` and resolve all compile errors.
- [ ] Task 15. Run `cargo test --workspace --lib` and verify all tests pass.
- [ ] Task 16. Kill any stale `nimblex-installer` process and relaunch the new binary.

## Verification Criteria

- `cargo build --release` produces no errors.
- All 21 unit tests pass.
- On a USB install with a 16 GB stick containing 6 GB of bundles:
  - Progress bar reaches ~7% after the 6 quick partition/format steps
    (approx `(1+0.5+0.5+0.5+0.5+2+3) / 88 × 100 ≈ 9.1%`)
  - Bar advances smoothly from 9% to ~89% during `copy-system`,
    updating every ~80 MB (2% of 4 GB)
  - Bar jumps to ~97% when bootloader step starts
  - Bar hits 100% on `Complete`
- `PROGRESS:XX` lines are not shown in the log pane.
- The `Show commands` modal still renders correctly (it reads
  `plan.steps` directly; `weight` is serialised but not displayed).

## Potential Risks and Mitigations

1. **`Step` struct used in many `Step { … }` literals across planner.rs**.
   Adding a non-optional field breaks compilation everywhere.  
   Mitigation: Tasks 12–13 explicitly address this. Run `cargo check -p
   installer-core` first before the full build.

2. **Alongside-Windows plan has different step count; weights must sum
   correctly**.  
   Mitigation: The GUI normalises by `total_weight`, so absolute values
   don't matter — only relative ratios. Any values that give copy-system
   ~60–75% of the bar are correct.

3. **Multiple bundles with varying sizes** — a 3 GB `02-Xorg` bundle after
   a 200 MB `01-Core` would show 97% copied before the second starts.  
   Mitigation: Task 4 computes `total_bytes_all` across all bundles before
   the loop starts, so the percentage is genuinely total-based.

## Alternative Approaches

1. **Emit a new `SubProgress { index, percent }` event** from the helper
   protocol instead of parsing stdout lines. Cleaner separation but
   requires changes in 4 files (events.rs, runner.rs, screen_install.rs,
   internal.rs). Not chosen because stdout parsing achieves the same result
   with fewer touchpoints and is already being tested.

2. **Time-based estimation** (estimate duration from disk speed, show a
   smooth animation regardless of actual progress). Rejected — fake
   progress is worse than accurate slow progress on a large install.
