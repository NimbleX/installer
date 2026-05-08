# Installer feedback round 9 — Destination & Install polish + helper bug fix

## Objective

Fix the runtime bug that prevents any install from starting, then polish the
two screens to match the reference frames: clearer partition labels, distinct
NOW vs AFTER hierarchy, distinct existing-Linux vs new-Nimblex colours, a
visible reclaim-slider minimum, and a properly styled failed-install state.

## Implementation Plan

### A. Helper / runtime bugs

- [ ] Task A1. In `crates/installer-helper/src/runner.rs::run_step`, detect
      `argv[0] == "nimblex-installer-helper-internal"` and replace the
      program path with `std::env::current_exe()`. Use
      `std::os::unix::process::CommandExt::arg0(...)` to keep the alias
      visible to the child's argv0-dispatch in `main.rs`. This unblocks all
      7 planner-emitted internal steps. Add a unit test that builds a Plan,
      runs `execute()` against it, and asserts `PlanAccepted` followed by at
      least one `StepStart` is emitted (verifying the spawn doesn't fail).

- [ ] Task A2. In `crates/installer-helper/src/internal.rs`, change every
      stub from `println!("(stub) would: ...")` + `Ok(())` to emitting an
      Error event ("not implemented yet — install is stub-only in v1") and
      a non-zero exit. The current behaviour silently "succeeds" without
      doing anything, which is dangerous once the helper actually runs.

- [ ] Task A3. Add a top-level CLI flag `--allow-stubs` to the helper. When
      absent, stubs error out (per A2). When present (used during dev),
      stubs print and succeed. The GUI never passes the flag.

### B. Partition-preview visual fixes

- [ ] Task B1. In `crates/installer-gui/src/widgets/partition_preview.rs`,
      add a third caption tier: at `segw >= 24` show a short role label
      (`ESP`, `MSR`, `RCV`, `WIN`, `LX`, `NX`) only; at `segw >= 60` show
      label + size; at `segw >= 110` show label + badge + size (current
      behaviour). Update `PartitionRole::short_label()` if needed.

- [ ] Task B2. Distinguish existing Linux from the new Nimblex partition.
      Add a new `SegmentAction::NewNimblex` colour ramp (vivid teal-mint
      `#3bd4a3` low/high) that overrides the role colour. Existing Linux
      partitions stay on the muted Linux ramp. This makes "the new green
      slice" unambiguously identifiable on the AFTER strip.

- [ ] Task B3. Mute the NOW strip. In `draw_now`, multiply each partition's
      saturated colour by 0.75 and the light colour by 0.85 before
      filling. The AFTER strip stays at full saturation. Effect: NOW reads
      as "this is the current state", AFTER pops as "this is what we will
      do".

- [ ] Task B4. Always render the action badge for `Resize`, `NewNimblex`,
      `NewEsp`, and `Delete` segments, even when the segment is too narrow.
      When `segw < 70`, draw the badge as a 50 px left-anchored ribbon that
      protrudes above the strip (negative y-offset of -14 px) with a
      leader line down to the segment. Add a small connector triangle so
      the user can tell which segment the badge refers to.

- [ ] Task B5. For Delete segments, render the cross-hatch in 2 px lines
      (currently 1.6 px) and reduce hatch spacing to 8 px so the "X" is
      legible at 60-px segment widths.

### C. Reclaim slider polish

- [ ] Task C1. In `crates/installer-gui/assets/style.css`, give the
      `scale.reclaim highlight` selector a `min-width: 12px` so the green
      fill remains visible when the value sits at the lower bound.

- [ ] Task C2. Add a tooltip on the slider showing `"Drag to give Nimblex
      more space (min {min_install} · max {max_keep})"`.

- [ ] Task C3. Add visible tick marks at `min_install` and at the
      "Recommended" position via `Scale::add_mark()`. Two ticks total, the
      recommended one labelled `Recommended` below the trough.

### D. Install screen visual fixes

- [ ] Task D1. In `crates/installer-gui/src/widgets/circular_progress.rs`,
      change the GtkBox spacing from 12 → 20 px and hide the caption label
      when its text is empty (`caption_label.set_visible(false)` initially;
      `set_visible(!caption.is_empty())` in `set_caption`).

- [ ] Task D2. In `crates/installer-gui/assets/style.css`, change
      `.progress-caption` from 11 px / 0.18 em letter-spacing uppercase to
      14 px / 0.02 em / regular case / colour `nx-fg-muted`. Drop the
      `font-weight: 700` (was: bold).

- [ ] Task D3. Add `.progress-track--error` and `.progress-arc--error` CSS
      tokens (red `#e5484d` family). When `HelperEvent::Error` fires, set
      a `failed` flag on `CircularProgress` that swaps the colours in
      `draw()`, replaces the percent label with `✕`, and hides the
      caption.

- [ ] Task D4. Style the title in the success / error states. Add
      `.title-success` (green check inline, `nx-accent` text) and
      `.title-error` (red X inline, red text) CSS classes. Apply via
      `title.add_css_class()` from `handle_event`.

- [ ] Task D5. Replace the leaking `/install/log` text on the log pane
      header with `"Activity log"` styled the same way. Trace the actual
      source — likely `LogPane::new()` setting an initial text — and
      remove or replace.

- [ ] Task D6. When the helper exits non-zero, the Close button label
      changes to "Close" (already correct) but a secondary "Copy log to
      clipboard" outline button appears next to it for bug reports.

### E. Footer & Show-commands polish

- [ ] Task E1. Move "Show commands" from a left-aligned footer link to a
      right-aligned `btn-secondary` next to Cancel. Order in the footer
      becomes: spacer | [Show commands] | [Cancel] | [Continue].

## Verification Criteria

- A real install (not dry-run) starts: helper spawns successfully and the
  first internal step runs; at minimum, no `os error 2` is reported.
- Without `--allow-stubs`, internal stubs surface as `Error` events; the
  install screen shows the failed state, not "Install complete".
- On a 6-partition Windows disk, every NOW segment ≥ 12 px wide shows at
  least a short label; the new Nimblex slice on AFTER shows a `NEW ·
  NIMBLEX` ribbon even when the slice itself is < 70 px.
- NOW partitions are clearly less saturated than AFTER ones at a glance.
- The new Nimblex slice is visually distinct from any pre-existing Linux
  partition (different hue, not just position).
- Reclaim slider shows a green fill at minimum value (>= 12 px).
- Install screen at 0 % shows no caption at all (no `/install/log` text).
- On `Error`, the ring renders red, the percent label is replaced with
  `✕`, and the title row picks up `.title-error`.
- Footer order matches the reference: Show commands button is on the
  right, no longer a faint link on the left.

## Potential Risks and Mitigations

1. **`CommandExt::arg0` is Unix-only** — fine for this installer (Linux
   only), but guard with `#[cfg(unix)]` for clarity.
   Mitigation: explicit `#[cfg(unix)]` block; no Windows build target.

2. **Stubs erroring out by default could break the existing dry-run UX**
   for folks who use `--dry-run` to preview. Mitigation: dry-run never
   executes internal steps (it just prints argv), so this is unaffected.
   Confirmed by re-reading `runner::dry_run`.

3. **NewNimblex colour might collide with the EFI teal** if both
   appear in the AFTER strip. Mitigation: pick a more saturated mint
   (`#33e0a8`) than EFI's teal (`#45b3c2`); test side-by-side.

4. **Always-rendered badges that protrude above the strip will collide
   with the strip caption ("AFTER INSTALL")** if the badge is on the
   leftmost segment. Mitigation: when a left-edge badge would collide,
   draw it inside the segment (current behaviour) regardless of width;
   only out-of-band ribbons for non-left segments.

## Alternative Approaches

1. **Compact partition table view as fallback** — if visual labelling
   keeps fighting available width, add an expandable text-table below
   the strips listing every partition with its action. Defer to a later
   round; the strip is the priority per the reference frames.

2. **Use libadwaita `AdwSpinner` instead of the custom Cairo circle**
   for the install screen — simpler, theme-consistent, but loses the
   percent-in-the-middle look. Reject.

3. **Swap the helper for plain shell scripts** — would simplify the
   stub problem but throws away the JSON event protocol and allowlist
   guard rails. Reject.
