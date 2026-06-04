# Crash-Safe In-Place Reinstall Onto the Running Live System

## Objective

Make "reinstall Nimblex in place onto the partition you booted from" a **supported, crash-safe v1 feature**, while making every other write path that touches the boot device fail safe.

The headline outcome: when the user boots their installed Nimblex partition and re-runs the installer, the default `ReuseNimblex` (no-format) path overwrites the system **in place without crashing the running session**, by writing each bundle to a temp file and atomically renaming it over the old one. The live loop device keeps reading the old (now-unlinked) inode's blocks until reboot, while the directory entry points at the new file for next boot.

Enabling prerequisite: the installer must reliably detect the live-backing block device and whether the system is running from RAM, so it can (a) choose the safe copy strategy on the live device, (b) block the genuinely unsafe modes (format / repartition of the live disk), and (c) allow normal truncating copies everywhere else.

## Context and Key Findings

- The `.lzm` bundles are squashfs modules loop-mounted read-only and paged in on demand into the running aufs/overlay union. Truncating one in place (current `File::create` at `crates/installer-helper/src/internal.rs:480-481`) corrupts blocks the kernel later reads → crash.
- The scanner does not exclude the boot device (`crates/installer-core/src/scan.rs:99-123`) and nothing detects it. `live_source_dirs()` returns paths but not the backing device (`crates/installer-core/src/install_size.rs:73-87`).
- The prior change made `ReuseNimblex` default to no-format in-place install (`crates/installer-core/src/planner.rs:509`), which makes the self-overwrite scenario reachable and low-friction.
- Atomic-rename only rescues the in-place, no-format, live-partition path. Format (`mkfs`) and disk repartitioning (`sgdisk`/`parted`) on the live disk remain fatal and MUST be gated by detection. Therefore detection is load-bearing regardless.
- Detection is unprivileged (reads `/proc/self/mountinfo`, `/sys/block`), so it lives in `installer-core` and is callable from the GUI with no allowlist change. The helper guard (root side) is the catastrophic-failure backstop.

## Implementation Plan

### Phase 1 — Live-device detection (enabling prerequisite, fail-safe)

- [x] Task 1. Create `crates/installer-core/src/live.rs` exposing a `LiveMedia` struct: `backing_partition: Option<PathBuf>`, `backing_disk: Option<PathBuf>`, `running_from_ram: bool`, plus `LiveMedia::detect() -> LiveMedia`. Rationale: keep detection beside `scan.rs` (read-only probing) and out of the pure planner.
- [x] Task 2. Resolve the device backing `live_source_dirs()` by parsing `/proc/self/mountinfo`: find the mount whose mountpoint is the longest path-prefix of the bundles directory; capture its source and fstype. Rationale: the `.lzm` files physically live on that mount's device — exactly what must be protected.
- [x] Task 3. Classify the source: `tmpfs`/`ramfs` (or non-`/dev` source) ⇒ `running_from_ram = true`, `backing_partition = None`; a real block partition ⇒ record it; a `/dev/loopN` ⇒ resolve `/sys/block/loopN/loop/backing_file` and recurse to the partition that holds that file. Rationale: copy2ram is safe; a live block source is the dangerous case; loop indirection must be unwound.
- [x] Task 4. Map the live partition to its parent disk: add `PKNAME` to `LSBLK_FIELDS` (`crates/installer-core/src/scan.rs:18`) or match the resolved partition path against scanned `Disk.partitions[].path`. Rationale: gating needs both "is this the live partition" and "is this the live disk".
- [x] Task 5. Add comparison helpers (`is_live_disk(&Path)`, `is_live_partition(&Path)`) and re-export `live` from `crates/installer-core/src/lib.rs:1-27`. Rationale: testable policy primitives shared by GUI and planner.
- [x] Task 6. Fail safe on uncertainty: if the backing device cannot be confidently resolved, return `running_from_ram = false` with a populated/"unknown-but-present" indicator that the gating layer treats as "potentially live". Rationale: an unresolved topology must never be assumed safe.

### Phase 2 — Crash-safe copy engine (the headline feature)

- [x] Task 7. Convert `copy_system_inner` (`crates/installer-helper/src/internal.rs:431-526`) to write-temp-then-atomic-rename for every destination file: write `dst.tmp` on the same filesystem, `fsync` the file, `fs::rename(dst.tmp, dst)`, then `fsync` the directory. Rationale: rename-over is the only way to overwrite a live-mounted file without corrupting the running system.
- [x] Task 8. Thread an explicit copy-mode into `cmd_copy_system` (e.g. a `--inplace-live` flag on the `copy-system` subcommand in `crates/installer-helper/src/internal.rs:54-58`): on the live device use atomic-rename and do NOT unmount the live partition; on a normal target keep the existing mount-at-`/tmp/nimblex-target` + truncate path. Rationale: only the live path needs the costlier strategy and the unmount suppression.
- [x] Task 9. Suppress/relax the `unmount-target` step for the live-partition reuse path (`crates/installer-helper/src/internal.rs:376-403`): never lazily unmount the mount backing the running union. Rationale: detaching the live mount mid-session breaks the overlay.
- [x] Task 10. Add a free-space pre-check for the live in-place path: verify the partition holds old + new bundles simultaneously (rename keeps old inode blocks until reboot); abort early with a clear message otherwise. Rationale: peak usage is ~2× modules on the live device.
- [x] Task 11. Make bundle copy idempotent/clean for the no-format path: atomically replace changed bundles and remove stale `*.lzm` / obsolete kernels+initrds in `nimblex64/` and `boot/` that are not part of the new set, using the same rename/unlink discipline. Rationale: a no-format reinstall must not leave mixed module versions that won't boot.

### Phase 3 — Planner + GUI integration

- [x] Task 12. Plumb the copy mode through the plan: when the `ReuseNimblex`/no-format target is the live partition, `InstallPlanner::plan_reuse` (`crates/installer-core/src/planner.rs:509`) emits the `copy-system --inplace-live` argv and the relaxed unmount step. Rationale: the plan must encode the safe strategy explicitly so the helper and "Show commands" stay in sync.
- [x] Task 13. Detect `LiveMedia` once per scan in `crates/installer-gui/src/screens/screen_destination.rs` `refresh()` (~`:330`, `:349-355`) and store it on `AppState` (`crates/installer-gui/src/state.rs`). Rationale: stable for the session; avoids per-disk recomputation.
- [x] Task 14. In `refresh_layout`, gate by mode when the selection touches the live device and `running_from_ram == false`:
  - In-place `ReuseNimblex` (no format) on the live partition ⇒ **allowed** via the crash-safe path; surface an informational note ("reinstalling in place; changes apply after reboot").
  - Format checkbox ticked on the live partition ⇒ **blocked** (mkfs cannot be made safe).
  - Erase / Alongside / FreeSpace on the live disk ⇒ **blocked** (repartitioning the running disk).
  Mirror the existing block style at `crates/installer-gui/src/screens/screen_destination.rs:489-500`.
- [x] Task 15. When `running_from_ram == true`, impose no restriction (the medium is safe to modify, including format/erase). Rationale: don't punish the correctly-booted user.
- [x] Task 16. Write clear user-facing strings: for blocked cases, "You started Nimblex from this drive, so it can't be reformatted/repartitioned while running. Reboot and choose 'Copy to RAM', or use a separate USB." For the allowed live in-place case, "Nimblex will be reinstalled onto the partition you're running from; the update takes effect after you reboot." Rationale: users need the remedy and the expectation, not just a verdict.
- [x] Task 17. Update `summary_one_line` (`crates/installer-core/src/plan.rs`) so the live in-place case reads accurately (e.g. "Reinstall Nimblex in place on the running partition (applies after reboot)."). Rationale: the confirm overlay must reflect the special path.

### Phase 4 — Root-side backstop (defense in depth)

- [x] Task 18. Add a helper pre-flight guard: before `mkfs.*` or a truncating `copy-system`, the helper independently resolves the live-backing device and refuses to operate on it unless the plan carries the `--inplace-live` (rename) mode or an explicit RAM-safe marker. Add the flag to the allowlist subcommand validation (`crates/installer-helper/src/allowlist.rs:258-277`). Rationale: the helper is the only component that writes; a GUI/planner bug must not be able to destroy the running system.

### Phase 5 — Tests

- [x] Task 19. Unit-test the `/proc/self/mountinfo` parser and source classification with fixtures: tmpfs source ⇒ from-RAM; `/dev/sdb1` source ⇒ live partition; `/dev/loop3` ⇒ resolved backing partition; unresolved ⇒ fail-safe. Rationale: safety-critical, must be verifiable without a live system.
- [x] Task 20. Unit-test the gating policy over synthetic `Disk` + `LiveMedia` combinations (live partition + not-from-RAM + no-format ⇒ allowed-inplace; + format ⇒ blocked; live disk + erase ⇒ blocked; from-RAM ⇒ unrestricted; non-live ⇒ normal). Rationale: lock the matrix against regressions.
- [x] Task 21. Unit-test that `plan_reuse` emits `copy-system --inplace-live` and the relaxed unmount step only for the live-partition no-format case, and the normal steps otherwise. Rationale: prove the plan encodes the strategy correctly.
- [x] Task 22. Add a copy-engine test for atomic-rename semantics on a tmpdir (temp file created, fsync, renamed over target, no truncation of the original path mid-write; stale files removed). Rationale: the rename discipline is the core safety mechanism.
- [x] Task 23. Manual/real-hardware test checklist (documented in the PR, not committed as a doc file): live boot without copy2ram → in-place reinstall completes with no I/O errors in the running session and correct files after reboot; with copy2ram → format/erase allowed; free-space-exhaustion path aborts cleanly.

## Verification Criteria

- On a system booted WITHOUT copy2ram: selecting the live Nimblex partition with no-format performs a crash-safe in-place reinstall (no I/O errors live; new bundles present after reboot); ticking Format, or selecting Erase/Alongside/FreeSpace on the live disk, disables Continue with the remedy message.
- On a system booted WITH copy2ram: all modes on the live device are allowed and succeed.
- The helper refuses (clear non-zero error) to `mkfs` or truncating-`copy-system` the live-backing device when invoked directly without the RAM-safe/`--inplace-live` marker.
- The atomic-rename copy never truncates a destination in place; stale bundles are removed; the free-space pre-check aborts before writing when space is insufficient.
- All Phase 5 unit tests pass on CI (where no live mount exists, detection yields no false blocks).

## Potential Risks and Mitigations

1. **Unusual mount/loop topology (dm-crypt, nested overlays, netboot) mis-resolves the live device.**
   Mitigation: fail safe (Task 6) — unresolved ⇒ treat as potentially-live-and-not-RAM, block destructive modes, and use the rename path for any live-suspected copy.
2. **Atomic-rename bug corrupts the running OS (catastrophic, non-deterministic).**
   Mitigation: rename discipline isolated in one function with unit tests (Task 22); helper-side backstop (Task 18); mandatory real-hardware checklist (Task 23) before release.
3. **Live partition runs out of space mid-install due to 2× peak.**
   Mitigation: free-space pre-check (Task 10) aborts before any write; message suggests a larger target or reboot-to-RAM + format.
4. **Suppressing `unmount-target` on the live path leaves a second mount lingering.**
   Mitigation: write via the existing live mount (or a read-only bind for source) rather than a second rw mount; only suppress unmount for the live partition, keep current behaviour elsewhere.
5. **`running_from_ram` false-positive enables an unsafe format.**
   Mitigation: require both a tmpfs/ramfs bundles mount AND absence of a live block source; corroborate with the helper backstop.
6. **Over-blocking frustrates legitimate same-disk installs.**
   Mitigation: blocks apply only when not running from RAM; the in-place reinstall path stays allowed; messages explain the copy2ram remedy.

## Alternative Approaches

1. **Detection + gating only (no atomic-rename).** Block the live device unless copy2ram; user reboots to RAM or uses another USB. Simpler and fully safe, but does not deliver in-place reinstall — rejected as the v1 goal per the decision to support in-place reinstall.
2. **Exclude the live device from the picker entirely.** Simplest, but blunt: kills the copy2ram reinstall workflow and confuses users by hiding their drive — rejected.
3. **Boot-to-RAM requirement enforced at boot menu instead of in the app.** Out of scope for the installer and not always under our control — rejected, though documenting the copy2ram option in the boot menu is a complementary, low-cost improvement.

## Handoff Note

This plan is read-only analysis produced in planning mode. Implementation (file edits, builds, tests) must be carried out by an implementation agent (Forge). Suggested execution order: Phase 1 → Phase 2 → Phase 4 (backstop) → Phase 3 → Phase 5, landing detection and the root-side guard before exposing the in-place path in the GUI.
