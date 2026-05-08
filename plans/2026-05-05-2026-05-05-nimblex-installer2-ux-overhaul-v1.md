# Nimblex Installer UX/UI Overhaul

## Objective
Transform the installer from a functional utility into a premium, modern experience. Focus on direct manipulation, semantic clarity, and reducing cognitive load.

## Implementation Plan

- [ ] Task 1. **Color Semantics & CSS Update**: Update `tokens.css`. Assign distinct colors to avoid clashes: Windows (Azure `#2563eb`), New Nimblex (Mint `#3bd4a3`), Existing Linux (Purple `#8b5cf6`), System/EFI (Slate `#475569`). 
- [ ] Task 2. **Header & Footer Cleanup**: Move "Show commands" from the footer to a subtle terminal icon button (`>_`) in the `AppShell` header. Remove the floating summary text above the footer to keep the action area pristine.
- [ ] Task 3. **Disk Cards Enhancement**: Add a miniature capacity/usage bar inside `DiskCard` (under the meta text) to show disk usage at a glance. Add a Windows icon `🪟` to the Windows badge for immediate recognition.
- [ ] Task 4. **Single Interactive Partition Strip**: Replace the visually heavy "NOW" and "AFTER" strips with a single, taller "Planned State" strip in `PartitionPreview`.
- [ ] Task 5. **Direct Manipulation (Draggable Splitter)**: Remove the disconnected `reclaim` slider widget. Add a `gtk4::GestureDrag` to the `PartitionPreview` widget. Draw a tactile drag handle (`||`) at the boundary between the shrinking Windows partition and the new Nimblex partition. Update `requested_reclaim_bytes` dynamically as the user drags.
- [ ] Task 6. **Confirmation Overlay Polish**: Move the human-readable summary text (e.g., "Erase /dev/sda...") into the confirmation overlay as a clear, bold headline directly above the "I understand" checkbox.
- [ ] Task 7. **Error Screen Recovery**: On `ScreenInstall`'s failed state, replace the aggressive giant red circle with a refined, smaller warning icon. Add a "Copy Log" button (copies text to clipboard) and a "Back" button (resets state and returns to Destination screen) to prevent dead-ends.

## Verification Criteria
- [ ] Disk cards display a mini usage bar and distinct Windows icons.
- [ ] Only one partition strip is visible, representing the planned state.
- [ ] The boundary between Windows and Nimblex can be dragged directly with the mouse to resize.
- [ ] Existing Linux partitions are visually distinct (purple) from the new Nimblex partition (mint).
- [ ] The "Show commands" action is accessible via a header icon.
- [ ] The error screen allows copying the log and returning to the previous screen.

## Potential Risks and Mitigations
1. **Risk**: Implementing `GestureDrag` on a custom `DrawingArea` can be tricky with coordinate mapping.
   **Mitigation**: Map the X coordinate to a byte offset using the total disk size and the widget's allocated width. Clamp the drag to the safe minimum/maximum bounds defined by `ResizePlanner`.
2. **Risk**: Removing the "NOW" strip might lose context for power users.
   **Mitigation**: Ensure the new Nimblex partition has a clear `NEW` badge and the shrinking Windows partition explicitly shows `RESIZE`, making the transformation obvious.

## Alternative Approaches
1. If direct manipulation (Task 5) proves too complex in GTK4, fallback to a heavily styled slider that sits *flush* against the bottom of the partition strip, visually aligned with the resizable segments.