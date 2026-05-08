# Nimblex installer — modern dark-theme redesign (2 screens)

## Objective

Replace the current three-screen, light, button-heavy UI with the modern dark-theme single-card design shown in the four reference screenshots. Collapse the flow to **two screens** by merging disk-picking, alongside/erase mode, partition preview and the resize slider into one screen, with confirmation handled inline (checkbox + modal) before transitioning to the install-progress screen.

The visual language is non-negotiable: deep navy background, single rounded card centered in the viewport, teal-mint accent, stepper top-right, footer with "Renunță" link + pill-shape primary action.

## Reference frames (authoritative)

| Frame | Content |
|---|---|
| 1 (Screenshot 23-55-25) | USB picked — partition preview shows EFI · Data (~22 GB) · NimbleX (10 GB); slider value 10 GB. |
| 2 (Screenshot 23-55-55) | Internal disk picked — partition preview shows EFI · Windows (C:) (~348 GB) · NimbleX (163 GB). |
| 3 (Screenshot 23-56-08) | Install screen at 25 % — circular progress + log block "Pregătire partiții…". |
| 4 (Screenshot 23-56-18) | Install screen at 76 % — log block "Instalare GRUB bootloader…". |

The plan is locale-agnostic; ship strings in English by default, externalise for later l10n. The screenshots happen to be in Romanian — treat their text as illustrative copy intent, not mandatory wording.

## Design tokens (single source of truth)

Implement once in CSS, reference everywhere.

| Token | Value | Used for |
|---|---|---|
| `nx-bg` | `#0a0f1c` | window background outside the card |
| `nx-bg-card` | `#0f1626` | the main card |
| `nx-bg-elev` | `#152038` | nested panels (partition-preview frame, log block) |
| `nx-bg-elev-2` | `#1c2945` | hover/inactive segmented buttons |
| `nx-fg` | `#e6ecf5` | primary text |
| `nx-fg-muted` | `#8895a8` | secondary text, captions |
| `nx-fg-dim` | `#5a6678` | disabled text, info-icon |
| `nx-frame` | `#1f2a44` | card and panel borders |
| `nx-frame-strong` | `#2c3a5c` | hover/focus borders |
| `nx-accent` | `#3bd4a3` | primary accent (teal-mint) |
| `nx-accent-hover` | `#4fe4b3` | hover state of accent |
| `nx-accent-soft` | `#1a4438` | translucent accent fill (selected card bg) |
| `nx-windows` | `#3a7afe` | partition-preview Windows segment |
| `nx-windows-soft` | `#1a2c5a` | partition-preview Windows trough |
| `nx-protected` | `#3a4660` | EFI/MSR/Recovery segments |
| `nx-danger` | `#ff6b6b` | destructive copy / "Erase whole disk" subtle hint |
| Radius | `card 16px`, `panel 12px`, `button 10px`, `pill 999px` | |
| Shadow | `0 12px 40px rgba(0,0,0,0.45)` | card drop shadow |
| Type | sans 14 base, 13 caption, 22 H1, 13 stepper | system default sans |
| Spacing grid | 4 / 8 / 12 / 16 / 24 / 32 | strict adherence |

Color-blind contrast notes: teal/blue distance is sufficient (different hue + brightness) per WCAG. Verify with a CVD simulator at design-review time.

## Information architecture

**From three screens to two.** Confirmation becomes an inline mechanism on Screen 1, not a dedicated screen.

```
Screen 1: Destination (single card, scrollable inside if needed)
├── Stepper header        1. Destination · 2. Confirm · 3. Install
├── H1 + subtitle         "Where shall we install Nimblex?"
├── Disk cards row        [Samsung SSD …] [Kingston DataTraveler]
├── Mode toggle           [Install alongside (Resize)]  [Erase whole disk]
├── Partition preview     │EFI│ Windows (C:) (~348 GB) │ NimbleX (163 GB) │
├── Slider                "Space for Nimblex"           163 GB
├── Info row              ⓘ Drag to allocate. Original partition keeps its data.
├── Understand checkbox   ☐ I understand my disk will be modified.
└── Footer                "Cancel"            [Show commands]  [Continue ›]

Screen 2: Install
├── Stepper header        1. Destination · 2. Confirm · 3. Install (active)
├── Circular progress     76% / INSTALLING
├── Log block             > Mounting AUFS modules…
└── Footer                ⏳ Please wait…
```

The stepper still shows three logical phases (Destination, Confirm, Install) so the user mental-models a familiar progression, but the **Confirm** dot becomes "active" only briefly during the click of "Continue" on Screen 1, transitioning visually to Install. There is no dedicated Confirm screen.

A "Show commands" button on Screen 1's footer opens a modal with the existing `plan.shell_transcript()` output. Identical safety guarantee as today: snapshot of the same argv that will execute.

The Understand checkbox gates the **Continue** button (was previously the Install button on Screen 3). Click → Continue triggers a brief modal-style overlay confirming the destructive summary in one sentence ("This will shrink Windows to X and create Nimblex in Y. Proceed?") with two pill buttons; clicking the primary one navigates to Screen 2.

This collapses three navigational screens into two while preserving all current safety semantics (visible plan, explicit consent, dry-run-equivalent transcript).

## Component inventory

| Component | New / refactor | Notes |
|---|---|---|
| `AppShell` | new | Centered card, fixed max-width 880 px, drop shadow, stepper-header partial. |
| `StepperHeader` | new | Avatar "N" badge + title left, three dot-separated steps right, current in `nx-accent`. |
| `DiskCard` | new (replaces `disk-row`) | Icon + Model + "size · path" caption; selected = teal border + soft fill. Internal disks: HDD/SSD/NVMe icon. Removable: USB icon. |
| `SegmentedToggle` | new | Two-button toggle "Alongside / Erase". Selected = teal solid; other = `nx-bg-elev-2` outline. Disabled when scenario forbids (e.g. removable picked → Erase forced; internal without Windows → Alongside disabled). |
| `PartitionPreview` | refactor of `DiskStripView` | Single 56-px-tall bar, rounded segments separated by 2 px gaps, label inside each segment, no diagonal stripes (replace with a soft pulsing teal shimmer for unknown-usage), no "select partition" interaction — the partition is implied by the toggle mode + (for Alongside) the Windows partition is auto-picked as the largest NTFS. |
| `ReclaimSlider` | refactor | Track = `nx-windows-soft`, fill = `nx-accent`, thumb = white circle with teal ring. Right-aligned value label in teal. Hidden in Erase mode. |
| `InfoRow` | new | Outline info-icon + caption, `nx-fg-muted`. |
| `UnderstandCheckbox` | refactor | Square check, teal when active, label "I understand my disk will be modified." |
| `Footer` | new | "Cancel" as text link (left), "Show commands" outline button + "Continue ›" primary pill (right). |
| `ConfirmOverlay` | new | Modal-style sheet over the card, single sentence + two pill buttons. |
| `CircularProgress` | new | 220 px diameter, 14 px stroke, teal arc on dark ring, percent in big light text, "INSTALLING" beneath in muted caps. |
| `LogPane` | refactor | Black-card monospace block, teal `>` prefix per line, last line in teal. Limited to last ~6 visible lines, full transcript expandable on click of the `>_` icon. |
| `WaitFooter` | new | Spinner + "Please wait…" caption, replaces the action footer during install. |

## Implementation Plan

- [ ] Task 1. **Lock the design tokens.**
  Create `crates/installer-gui/assets/tokens.css` containing only the `@define-color` declarations and a small set of size/radius variables. Import it from `style.css`. Rationale: every later task references these names; defining them once prevents drift between the refactor PRs.

- [ ] Task 2. **Replace the global stylesheet.**
  Rewrite `crates/installer-gui/assets/style.css` from scratch using only the new tokens. Drop every selector tied to the old light theme (`welcome-page`, `picker-page`, `confirm-page`, `step-icon--*`, `bottom-bar`, `scenario-card`, etc.). The new stylesheet groups rules by component, matching the `Component inventory` section. Rationale: the old CSS encodes too many obsolete decisions; a clean rewrite is faster and safer than incremental edits.

- [ ] Task 3. **Build the `AppShell` + `StepperHeader`.**
  New module `crates/installer-gui/src/widgets/app_shell.rs`. Owns the outer `Window`, draws the dark backdrop, hosts a centered `Box.app-card` of fixed max-width, mounts a `Stack` for the two screens. The stepper at top-right is a small horizontal box with three children, each `<idx>. <label>`, separated by `·` characters; the active step's foreground is `nx-accent`. The stepper exposes a `set_active(StepId)` method. Rationale: every screen lives inside the same chrome; centralising it eliminates per-screen padding bugs.

- [ ] Task 4. **Implement `DiskCard` and the two-card row.**
  New module `crates/installer-gui/src/widgets/disk_card.rs`. The card contains: a 40 px icon (NVMe/SSD/HDD/USB based on `disk.transport` + `removable`), a bold model line, a muted "size · /dev/path" caption. Cards use `:checked` state via `ToggleButton` with `set_group(Some(&first_card))` so radio behaviour comes for free. The selected card gains `nx-accent` border + `nx-accent-soft` fill. Rationale: `ToggleButton` group semantics handle keyboard navigation and exclusivity natively; reinventing it via `GestureClick` was a source of bugs in the previous design.

- [ ] Task 5. **Implement `SegmentedToggle` for install mode.**
  New module `crates/installer-gui/src/widgets/segmented_toggle.rs`. Two `ToggleButton`s in a group, fixed-width container, the selected one gets `.toggled` styling. Public API: `connect_changed(FnMut(InstallMode))`, plus `set_enabled(InstallMode, bool)` to disable individual options. The widget hides itself when no disk is selected. Rationale: the alongside/erase decision is a single binary that should look like a binary.

- [ ] Task 6. **Refactor `DiskStripView` into `PartitionPreview`.**
  Strip the click-to-select gesture, the "selected" highlight, the lock icon, the diagonal-stripe fallback, and the per-partition usage sub-fill. Keep only the role-coloured segmented bar with rounded corners and an in-segment label `"Windows (C:) (~348 GB)"`. Add a `set_planned_split(LayoutPreview)` method that takes the post-resize layout (existing partitions kept + new NimbleX segment in `nx-accent`). The widget is read-only: clicking does nothing. Rationale: the new design uses the preview for *visualisation*, not for selection — the click-to-select model is gone with the unified Screen 1.

- [ ] Task 7. **Auto-detect the resize target instead of asking the user to click.**
  Add `Disk::primary_windows_partition()` and `Disk::primary_data_partition()` helpers in `installer-core` that pick the largest non-protected NTFS / largest non-protected partition respectively. When the user toggles to "Alongside (Resize)", the planner uses this partition automatically; when toggled to "Erase whole disk", the slider hides. Rationale: in the screenshots there is no per-partition selection — the Continue button is enabled the moment a disk + a mode are chosen. This matches the user's intent of "we can even have 2 screens".

- [ ] Task 8. **Restyle `ReclaimSlider`.**
  Rewrite the CSS targeting `scale.reclaim trough/highlight/slider`. Track in `nx-windows-soft`, highlight in `nx-accent`, thumb 20 px white circle with 3 px `nx-accent` ring. Right-aligned live value label using Pango markup `<span foreground='@nx-accent' weight='600'>163 GB</span>`. The label "Space for Nimblex" sits left of the value on the same row. The slider hides entirely in Erase mode. Rationale: matches the screenshot exactly; reuses existing `connect_value_changed` plumbing.

- [ ] Task 9. **Inline `InfoRow` and `UnderstandCheckbox`.**
  Rebuild the bottom of Screen 1: an `ⓘ` info row with the explanatory caption (visible only in Resize mode), then the Understand checkbox. The checkbox text: "I understand my disk will be modified." When unchecked, the **Continue** button is desensitised. Rationale: the Understand gate moves from Screen 3 onto Screen 1 because Screen 3 is gone.

- [ ] Task 10. **Build the new `Footer` for Screen 1.**
  Three children: "Cancel" as a flat text-style button (left, `nx-fg-muted`), "Show commands" as outline-pill button, "Continue ›" as filled-pill primary (right). Cancel quits the application after a small confirmation. The chevron `›` in the primary button is a separate `Image` from `pan-end-symbolic` so the symbol stays crisp at any DPI. Rationale: footers are the highest-visibility actionable area; every pixel matters.

- [ ] Task 11. **Add `ConfirmOverlay`.**
  When the user clicks Continue, an overlay sheet animates up from the bottom of the card (use `Revealer` with `SlideUp`). It shows: a one-sentence summary derived from `Plan.summary_one_line()` (new method to add to `installer-core::plan`), and two pill buttons "Back" / "Install now". "Install now" triggers the existing `helper --run` path. Rationale: confirmation is a momentary modal, not a screen.

- [ ] Task 12. **Build `CircularProgress`.**
  New `crates/installer-gui/src/widgets/circular_progress.rs` using `DrawingArea` + Cairo. Two arcs: a faint `nx-frame-strong` ring as background, a teal arc from -90° clockwise to `(value × 360°) - 90°`. Centre text rendered with Pango at 36 pt for the percent and 11 pt uppercase for the caption. Public API: `set_value(f64 in 0.0..=1.0)`, `set_caption(&str)`. Animates value transitions over 400 ms via `gtk4::TickCallback` + ease-out cubic. Rationale: the install screen's hero element; deserves a dedicated widget.

- [ ] Task 13. **Refactor `LogPane`.**
  New module `crates/installer-gui/src/widgets/log_pane.rs`. A `ScrolledWindow` wrapping a `TextView` set to monospace, dark background, no-edit. Each line is prefixed with `> ` in `nx-accent`. The most recent line uses `nx-accent`, prior lines use `nx-fg-muted`. A small `>_` icon in the top-right corner toggles between "last 6 lines" view and "full transcript" view. Rationale: the screenshot's log block is iconic and trivial to reproduce; the visual `>` cue maps to the existing `StepCategory` markers.

- [ ] Task 14. **Build `WaitFooter`.**
  Spinner + "Please wait…" caption, replaces the action footer when Screen 2 is visible. Auto-hides when the helper emits its terminal `done` event, then a "Reboot" / "Close" pill row replaces it. Rationale: simpler footer state machine than embedding everything in one stack.

- [ ] Task 15. **Wire helper progress events into the new widgets.**
  Modify `crates/installer-gui/src/helper.rs` to spawn `pkexec installer-helper --run`, parse the JSON event stream line-by-line on a glib `MainContext::channel`, and route each event to: `CircularProgress::set_value(running_step / total)`, `LogPane::append_line(step.label)`, `StepperHeader::set_active(Install)`. Use `glib::clone!(@strong …)` to share GTK widget refs into the channel receiver. Rationale: the existing event protocol already carries everything we need; this is purely a wiring change.

- [ ] Task 16. **Delete the obsolete screen modules.**
  Remove `crates/installer-gui/src/screens/screen1_welcome.rs` (welcome cards merged into Screen 1's disk cards), `screen3_confirm.rs` (replaced by `ConfirmOverlay` + new install screen), and rename `screen2_picker.rs` → `screen_destination.rs`. Add `screen_install.rs`. Rationale: the file layout should reflect the two-screen reality; leaving stale modules invites accidental routing.

- [ ] Task 17. **Update `AppState` to match the new flow.**
  Drop `selected_partition_number` (no longer user-selected). Add `install_mode: Option<InstallMode>` (Alongside / Erase). Move `requested_reclaim_bytes` semantics so it's interpreted only when `install_mode == Alongside`. Rationale: the data model should mirror the UI's choices, no more, no less.

- [ ] Task 18. **Update planner glue.**
  In `installer-core::InstallPlanner`, expose `plan_for(disk, mode, optional_reclaim_bytes)` that internally calls `plan_usb` or `plan_alongside_windows` with the auto-picked Windows partition and a target derived from `disk.size − reclaim`. Rationale: collapses the GUI's decision tree to a single function call.

- [ ] Task 19. **Add `Plan::summary_one_line()`.**
  New method on `installer_core::Plan` returning `"Shrink Windows to 348 GB and install Nimblex (163 GB)."` or `"Erase /dev/sdb (Kingston DataTraveler, 32 GB) and install Nimblex."`. Used by the `ConfirmOverlay`. Rationale: the planner owns the canonical English description of what will happen.

- [ ] Task 20. **Visual QA pass against the four reference frames.**
  Build a small `make screenshots` target that launches the GUI, drives it programmatically through both screens with a fake disk fixture, and screenshots both states. Compare against the reference frames at design-review time. Rationale: prevents regression of the visual language as later features are added.

- [ ] Task 21. **Accessibility & input audit.**
  Verify: tab order is Disk1 → Disk2 → Toggle → Slider → ShowCmds → Cancel → Continue; Esc on Continue goes back; F5 refreshes disks; the slider is reachable and operable from the keyboard (arrow = step, PgUp/PgDn = page, Home/End = bounds); high-contrast users see a visible focus ring on every interactive element. Rationale: the new look should not regress accessibility relative to the old GTK defaults.

- [ ] Task 22. **Locale scaffolding.**
  Wrap every user-visible string in a `t!()` macro that today resolves to identity but is wired through `gettext-rs` in a follow-up. Mark all strings extractable. Rationale: the screenshots are Romanian; the architecture should make swapping a lang file trivial.

## Verification Criteria

- The running installer is visually indistinguishable, side-by-side, from the four reference frames at default-DPI 1× and 2× scale (verified by visual diff against `tests/snapshots/`).
- The flow contains exactly two navigational screens; the confirmation is a `Revealer`-based overlay on Screen 1, not a third screen.
- On Screen 1, picking a removable disk auto-disables the "Alongside" toggle option; picking an internal disk without Windows auto-disables the "Alongside" option; in either case the "Erase whole disk" option remains operable.
- The reclaim slider is hidden in Erase mode and visible in Alongside mode; in Alongside mode the partition preview shows three segments (EFI · existing · NimbleX) and they update live as the slider moves.
- Continue is desensitised until: a disk is selected AND a mode is chosen AND (in Alongside mode) the reclaim is within `[min_reclaim, volume − win_kept_floor]` AND the Understand checkbox is checked.
- The Show-commands modal renders the exact same `plan.shell_transcript()` argv as the helper would execute (re-uses existing snapshot tests in `installer-core::plan::tests`).
- On Screen 2 the circular progress animates smoothly between values, the log pane scrolls, and the active step in the stepper is "3. Install".
- `cargo test --workspace --lib` continues to pass with at least the same number of tests as before the refactor; new widgets contribute their own unit tests for layout and state-transition logic.
- `ldd target/release/nimblex-installer` references only sonames present in `01-Core` + `02-Xorg` (existing CI gate, unchanged).
- Keyboard-only operation can complete the flow from launch to "Install now" without touching the mouse.
- No `RefCell::borrow()` chain holds an outer `Ref` while a closure inside takes a nested borrow (regression guard for the slider bug).

## Potential Risks and Mitigations

1. **Risk: GTK's default light theme leaks through some widgets (e.g. `Adjustment` popups, native dialogs) on systems where the user has a light system theme.**
   Mitigation: set `GTK_THEME=Adwaita-dark` via the `.desktop` file's `Exec` line and call `Settings::default().set_property("gtk-application-prefer-dark-theme", true)` during app startup. Snapshot tests run in both light and dark host environments.

2. **Risk: The teal accent loses contrast on very dim displays or for users with deuteranomaly.**
   Mitigation: WCAG AA verified against `nx-bg-card` (4.5:1 minimum). A high-contrast variant of the token set kicks in when `gtk-application-prefer-dark-theme` is forced and `gtk-theme-name` contains "HighContrast". Provide a CVD-simulator screenshot as part of the design-review checklist.

3. **Risk: `ToggleButton`-as-radio for the disk cards loses its group when the disks list refreshes.**
   Mitigation: hold the first card's `ToggleButton` ref in `Screen1` state; rebind subsequent cards to it in `refresh()`. Cover with a unit test that simulates a hot-plug refresh.

4. **Risk: The auto-pick of "primary Windows partition" is wrong on disks with multiple NTFS volumes (e.g. a backup partition larger than C:).**
   Mitigation: heuristic: among NTFS partitions, prefer the one whose label matches `^(C:|Windows|System|OS|Boot)$` (case-insensitive); break ties by `mountpoint == /` for the running Windows or by largest among unmounted. Document the heuristic as a comment with a TODO for explicit user override in v1.1.

5. **Risk: Hiding the partition picker behind an auto-pick reduces user agency for power users.**
   Mitigation: a tiny "Choose another partition…" link below the partition preview, gated by a `--advanced` CLI flag for v1, opens a small modal that lists all eligible partitions. Out of scope for v1's checkboxes above; tracked as a follow-up.

6. **Risk: The circular-progress animation on Cairo can stutter on weak hardware (e.g. live USB on an old laptop).**
   Mitigation: redraw at 30 Hz max; switch to step-jumps (no easing) when the system load average exceeds 1.0; budget 0.5 ms per redraw. Profile with `sysprof` on a 2014-era laptop before sign-off.

7. **Risk: The Confirm overlay can be dismissed by accident, putting the user back onto Screen 1 mid-progress (if the overlay is shown after Continue but Helper has started).**
   Mitigation: the overlay's "Install now" button starts the helper *and* immediately transitions to Screen 2; the overlay is destroyed during transition. There is no path where the helper is running while the overlay is still present.

## Alternative Approaches

1. **Keep three screens, only restyle.**
   Pro: minimal code churn, plan tasks shrink by ~30 %. Con: doesn't match the user's explicit request "we can even have 2 screens", retains the navigational complexity.

2. **Use `libadwaita` (`Adw.NavigationView` + `Adw.AlertDialog`) instead of hand-rolled widgets.**
   Pro: built-in dark-theme tokens and animations. Con: `libadwaita` is **not** in 01-Core / 02-Xorg per the project's hard dependency rule. Rejected on those grounds. Could be reconsidered if libadwaita is added in a future bundle.

3. **Single-screen design (no Screen 2).**
   Pro: even simpler. Con: install progress would need to overlay or replace the destination card, fighting the user's mental model of "I confirmed, now it's running"; harder to interrupt safely if needed. Rejected.

4. **Render the partition preview as a vertical bar with text labels beside it, like Disks (gnome-disk-utility).**
   Pro: handles >5 partitions without crowding. Con: doesn't match the screenshot, which is decisively horizontal; vertical eats more vertical real estate that the slider needs. Rejected.

5. **Animate transition between Screen 1 and Screen 2 with a shared-element morph (the disk-card "moves" into the circular progress).**
   Pro: visually delightful. Con: GTK's `Stack` transitions are limited to slide/fade; shared-element requires manual `DrawingArea` choreography. Out of scope for v1; revisit when polish budget allows.
