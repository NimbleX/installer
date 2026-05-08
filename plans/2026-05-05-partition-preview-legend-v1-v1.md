# Partition Preview — Minimum Width + Clean Legend

## Objective

Fix the partition strip so that:
1. No segment is ever rendered narrower than `MIN_SEG_PX = 30px`, regardless of how small it is relative to total disk size.
2. No text is drawn **inside** segments — the strip is purely coloured blocks.
3. A one-line colour-coded legend **below** the strip explains every segment.

---

## Implementation Plan

- [ ] Task 1. **Add `compute_visual` helper** in `crates/installer-gui/src/widgets/partition_preview.rs`.

  Two-pass algorithm:
  ```
  available_px = widget_width - 2  (1px border each side)
  Step 1: proportional_w[i] = (seg.size / total) * available_px
  Step 2: clamp each w[i] to max(w[i], MIN_SEG_PX)
          record deficit = sum of (MIN_SEG_PX - proportional_w[i]) for small segs
  Step 3: scale down large segs proportionally to absorb the deficit:
          scale = (elastic_sum - deficit) / elastic_sum
          for large segs: w[i] = max(w[i] * scale, MIN_SEG_PX)
  Step 4: accumulate x positions from left
  ```
  Return `Vec<(f64, f64)>` of `(x_offset, visual_width)` per segment.

- [ ] Task 2. **Remove all text rendering from `draw_after` / `draw_strip`.**

  Delete calls to `draw_caption_with_badge`, `draw_badge_only`, `draw_action_ribbon`.
  The strip function signature becomes: paint coloured rectangles + diagonal hatches + drag handle only.
  Keep the `RIBBON_GUTTER` constant removal (drop height back to `STRIP_HEIGHT = 56`).

- [ ] Task 3. **Update `draw_strip` to use `compute_visual` for positions.**

  Replace:
  ```rust
  let x = (s.start as f64 / total as f64) * w;
  let segw = (s.size as f64 / total as f64) * w;
  ```
  With:
  ```rust
  let (lx, segw) = layout[i];
  let px = 1.0 + lx;
  ```
  Add a thin 1px dark separator between adjacent segments at `px + segw`.

- [ ] Task 4. **Keep drag handle rendering** (`draw_drag_handle`) at the Resize→NewNimblex boundary. Use `layout[i].0 + layout[i].1` for the x position (not the proportional formula).

- [ ] Task 5. **Rewrite `update_legend`** to build Pango markup.

  For each segment in `planned`:
  - Skip segments where `action == Free && size < 10 MiB` (alignment gaps).
  - Emit: `<span foreground="#HEX">■</span> <b>Name</b>tag <span foreground="#5a6a8a">(Size)</span>`
  - Join entries with three spaces `   `.
  - Call `legend.set_use_markup(true)` and `legend.set_markup(&markup)`.

  Color mapping (`seg_hex_color`):
  | Condition | Hex |
  |---|---|
  | `NewNimblex` | `#3bd4a3` |
  | `NewEsp` | `#44b3c2` |
  | `Free` | `#2a3450` |
  | `WindowsSystem` / `WindowsData` | `#4a90e2` |
  | `EfiSystem` | `#44b3c2` |
  | `MicrosoftReserved` | `#475569` |
  | `WindowsRecovery` | `#6b7a99` |
  | `Linux` | `#3aa2c2` |
  | `LinuxSwap` | `#f5b800` |
  | `Other` | `#576080` |

  Legend tag per action:
  | Action | Tag |
  |---|---|
  | `Keep` | *(empty)* |
  | `Resize` | `" → resized"` |
  | `Delete` | `" → deleted"` |
  | `NewNimblex` | `" new"` |
  | `NewEsp` | `" new"` |
  | `Free` | *(empty)* |

- [ ] Task 6. **Remove unused helpers**: `draw_caption_with_badge`, `draw_badge_only`, `draw_action_ribbon`, `badge_color_for`, `needs_ribbon`. This eliminates all dead-code warnings for this file.

- [ ] Task 7. **Update `PartitionPreview::new()`** — remove the `RIBBON_GUTTER` from `height_request` and `content_height`. Set `legend.set_use_markup(true)` during construction.

- [ ] Task 8. **Build, kill stale process, relaunch.**
  ```
  cargo build --release 2>&1 | tail -5
  pkill -9 -f nimblex-installer
  ./target/release/nimblex-installer &
  ```

---

## Verification Criteria

- A 1 GiB Recovery partition on a 500 GiB disk renders at least 30px wide — visibly distinct, not a pixel.
- No text appears inside the strip at any zoom level.
- Legend line appears below the strip and contains one `■ Name (size)` entry per segment (excluding tiny alignment gaps).
- Each legend swatch colour exactly matches its segment colour in the strip.
- Drag handle between the Windows (Resize) and Nimblex (New) segments still works and fires `set_on_reclaim_changed`.
- Free space is still shown in the strip as a diagonal-hatch pattern, and appears in the legend as `■ Free (X GB)` only if ≥ 10 MiB.
- `cargo build --release` produces no new warnings.

## Potential Risks and Mitigations

1. **`compute_visual` overcorrects on disks with many small partitions** — if there are 10+ small partitions each requiring `MIN_SEG_PX`, the elastic large-segment budget goes negative. Mitigation: clamp the scale factor to `max(scale, 0.0)` and accept that large segments may be reduced to `MIN_SEG_PX` in extreme cases. This is still correct — the strip becomes "not proportional" but remains readable.
2. **Pango markup escaping** — partition labels may contain `<`, `>`, `&`. Mitigation: run labels through a simple `glib::markup_escape_text` call (or manual `replace("&", "&amp;").replace("<", "&lt;")`) before embedding in the markup string.
3. **`legend.set_use_markup(true)` must be called before `set_markup`** — GTK silently falls back to plain text if the flag isn't set. Set it in `PartitionPreview::new()`, not lazily.

## Alternative Approaches

1. **Draw the legend as a second Cairo `DrawingArea`** — gives pixel-perfect coloured squares (not Unicode `■`), but requires more code. The Pango markup approach is simpler and the `■` glyph renders well in all system fonts.
2. **Show only changed segments in the legend** (skip `Keep` entries) — reduces clutter on disks with many unchanged partitions. Can be toggled with a "Show all" link. Defer to v1.1.
