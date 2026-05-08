# Nimblex Installer UX Polish & Bug Fixes

## Objective
Fix the locked resize slider bug, refine the typography and icons to look truly modern, and improve the visual polish of the drag handle and error screens.

## Implementation Plan

- [ ] Task 1. **Fix the Resize Slider Math**: In `crates/installer-core/src/resize.rs`, remove the `HEADROOM_MULT` constant and its usage in `min_windows_residual_after_shrink`. Relying solely on `WINDOWS_FREE_FRACTION_AFTER_SHRINK` (10% free space guarantee) is mathematically sound and will unblock the slider.
- [ ] Task 2. **Modernize Typography**: In `crates/installer-gui/assets/style.css`, update the font-family to a robust modern stack: `system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif`. This guarantees crisp rendering even if specific fonts like Inter are missing.
- [ ] Task 3. **Refine Disk Card Icons**: In `crates/installer-gui/src/widgets/disk_card.rs` and `style.css`, remove the boxy `.disk-card-badge` background. Increase the `Image` icon size to 32px and color it `nx-fg-muted` (unselected) or `nx-accent` (selected) for a cleaner, unboxed look.
- [ ] Task 4. **Subtle Windows Badge**: In `style.css`, change the `.win-pill` from a solid blue background to a hollow outline style (e.g., `border: 1px solid @nx-windows; color: @nx-windows; background: transparent;`).
- [ ] Task 5. **Enhance Drag Handle Visibility**: In `crates/installer-gui/src/widgets/partition_preview.rs`, redraw the drag handle. Instead of thin lines, draw a solid white rounded rectangle (e.g., 6px wide, 24px tall) with a subtle black drop shadow so it clearly communicates "draggable".
- [ ] Task 6. **Polish Error Screen**: In `crates/installer-gui/src/widgets/circular_progress.rs`, replace the raw `⚠` emoji with a proper GTK symbolic icon (`dialog-warning-symbolic` or similar) using an `Image` widget, or style the text more cleanly. Reduce the aggressive font size of the "Install failed" title.
- [ ] Task 7. **Improve Partition Text**: In `partition_preview.rs`, increase the font size of the segment labels (`WIN`, `ESP`, etc.) from 9px to 11px and ensure they are vertically centered perfectly.

## Verification Criteria
- [ ] The user can drag the partition boundary to reclaim more space than the minimum.
- [ ] Disk card icons are unboxed and scale nicely.
- [ ] The drag handle looks like a physical, clickable thumb.
- [ ] The error screen uses a clean, native warning icon instead of an emoji.
- [ ] Fonts look modern and crisp across the entire application.

## Potential Risks and Mitigations
1. **Risk**: Removing `HEADROOM_MULT` might leave Windows with too little space if the drive is nearly full.
   **Mitigation**: The `WINDOWS_FREE_FRACTION_AFTER_SHRINK` ensures Windows always gets at least 10% of its *post-shrink* size as free space, plus the absolute `WINDOWS_RESIDUAL_FLOOR` of 40 GiB. This is highly safe.
2. **Risk**: GTK Cairo drop shadows can be complex to draw.
   **Mitigation**: A simple pseudo-shadow can be achieved by drawing a black rounded rectangle at `x+1, y+1` with 0.3 alpha, followed by the white rectangle at `x, y`.