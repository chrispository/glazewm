# Smooth Win+Drag Resize Plan

## Goal

Make Win+right-drag resizing track the cursor smoothly on Windows, while keeping ordinary focus-follows-cursor behavior inexpensive and preserving the existing tiling/floating resize semantics.

The implementation should be staged so the high-value, low-risk cadence fix can be tested independently from the more invasive multi-window batching work.

## Confirmed causes

### 1. Resize is currently capped at 20 updates per second

`packages/wm-platform/src/platform_impl/windows/mouse_listener.rs` throttles every `MouseEventKind::Move` to one event per 50 ms (`Duration::from_millis(50)`). That throttle predates Win+drag and was intended for focus-follows-cursor, where 20 Hz is adequate.

Win+right-drag now uses that same stream as its resize clock in `packages/wm/src/events/handle_mouse_move.rs::handle_resize_move`. Consequently, the window only receives approximately 20 geometry updates per second. At a cursor speed of 1,000 px/s, each update can jump about 50 px.

### 2. Tiled siblings are repositioned separately

Each resize event updates tiling sizes, queues the parent children for redraw, and immediately runs `platform_sync`. `redraw_containers` then calls `SetWindowPos` separately for every affected window, using `SWP_ASYNCWINDOWPOS`. Adjacent windows can therefore present their new frames at different moments, briefly producing gaps or overlaps.

### 3. Self-generated location events are not identified as part of modifier-resize

Win+right-drag is tracked in `WmState::drag_resize`, but it does not set the window's existing `active_drag` property. Each `SetWindowPos` generates `EVENT_OBJECT_LOCATIONCHANGE`, and `handle_window_moved_or_resized` consequently processes those notifications as ordinary external window changes. This is mainly unnecessary work for tiled windows and can race with floating placement updates.

### 4. Button-up does not apply the final cursor position

The right-button-up branch clears `drag_resize` without calling `handle_resize_move` using the button-up event's position. If the last move was throttled, the final window boundary can remain behind the cursor by up to one throttle interval.

## Phase 1: Fix input cadence and final-position accuracy

This phase should be one focused commit and should be tested before attempting batching.

### A. Use a 16 ms move interval on Windows

In `packages/wm-platform/src/platform_impl/windows/mouse_listener.rs`:

- Replace the hard-coded 50 ms move interval with a named constant, for example `MOUSE_MOVE_INTERVAL: Duration = Duration::from_millis(16)`.
- Use the constant in the move-emission check.
- Do not change the macOS listener as part of this Windows-only fix.
- Preserve the current behavior of reading the current absolute cursor position with `GetCursorPos`. Intermediate raw-input messages may be dropped; the next emitted event still catches up to the real cursor and does not accumulate deltas.
- Preserve unconditional delivery of button down/up events.

Sixteen milliseconds gives a safe approximately 60 Hz baseline. Do not start at 8 ms: many applications perform expensive synchronous layout during resizing, and 120 Hz could create unnecessary CPU load before measurements justify it.

### B. Apply one final resize on right-button release

In `packages/wm/src/events/handle_mouse_move.rs`, change the active-resize `MouseEvent::ButtonUp` match arm to bind `position` and:

1. Call `handle_resize_move(position, state)` while `state.drag_resize` is still populated.
2. Clear `state.drag_resize` afterward, even if the final target matches the previous target.
3. Let the normal end-of-event `platform_sync` apply the queued redraw.

Be careful about error handling: avoid leaving a stale resize active if the final resize returns an error. A robust structure is to store the result, clear the state, then propagate the result.

### C. Avoid redundant redraws at an unchanged target

Before adding new state, measure whether duplicate absolute cursor positions occur frequently at 16 ms. If they do, add the last applied target rectangle or cursor position to `DragResize` and skip `set_window_size`/redraw when unchanged. This is optional for the first commit; correctness and smoothness do not depend on it.

Do not compare only the requested target against `native_properties().frame`, because borders, DPI adjustment, layout gaps, and application-enforced minimum sizes can make the native frame differ from the model target.

### Phase 1 tests

- Keep the existing edge-selection, size-delta, and anchored-rectangle tests.
- Extract the throttle decision into a small pure/testable helper if necessary and test:
  - the first move is emitted;
  - a move before 16 ms is suppressed;
  - a move at or after 16 ms is emitted;
  - button transitions are never suppressed.
- Add a handler-level or factored-state test proving button-up applies its position before clearing `drag_resize`.
- Run:

  ```powershell
  cargo fmt --all -- --check
  cargo test -p wm
  cargo test -p wm-platform
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  ```

If `wm-platform` retains its custom test harness behavior, use the repository's supported package test invocation rather than converting its target structure.

### Phase 1 manual acceptance criteria

Test both floating and tiled windows with Win+right-drag:

- Motion visibly follows the cursor rather than stepping at 20 Hz.
- Releasing during a fast drag leaves the boundary at the release position.
- All four grabbed quadrants keep the opposite edges anchored.
- Minimum floating dimensions remain enforced.
- Normal mouse movement and focus-follows-cursor do not cause excessive CPU usage.
- Win+left-drag remains smooth and still retiles correctly on release.
- Right-click without Win continues to reach the application normally.
- Test at least one classic Win32 app and one Electron/Chromium app.

## Phase 2: Suppress self-generated resize feedback

This should be a separate commit so regressions can be isolated.

In `packages/wm/src/events/handle_window_moved_or_resized.rs`, after resolving the managed window and reading/updating its actual native frame, detect whether `state.drag_resize` references that window's container ID.

While that condition is true:

- Retain the native frame cache update.
- Return early before maximized/fullscreen detection, shadow-border refresh, floating-placement migration, or normal interactive-drag logic.
- Do not reuse `ActiveDrag` for this. `platform_sync::reposition_window` gives `ActiveDrag` special resize-only behavior that would be wrong for floating top/left anchoring and for sibling layout updates.

Delayed `EVENT_OBJECT_LOCATIONCHANGE` events that arrive after button-up may go through the normal path. That is acceptable: by then they describe the final applied geometry. If testing exposes stale post-release placement, add a narrowly scoped generation/token mechanism rather than a time-based blanket ignore.

### Phase 2 tests and acceptance criteria

- Factor the `drag_resize` window-ID check into testable logic if direct event-handler setup is too heavy.
- Verify external/native edge resizing still enters the existing `ActiveDrag` lifecycle.
- Verify programmatic resize commands outside modifier-drag still update state normally.
- Enable debug logs temporarily and confirm modifier-resize no longer runs the full moved/resized handler once per affected window per frame.
- Remove any temporary high-frequency logging before committing.

## Phase 3: Atomically batch tiled-window geometry updates

Only begin this after Phase 1 has been manually tested. Phase 1 should provide most of the perceived improvement; this phase addresses multi-window tearing.

### A. Add a Windows batch-position abstraction in `wm-platform`

Use `BeginDeferWindowPos`, `DeferWindowPos`, and `EndDeferWindowPos` from `Win32_UI_WindowsAndMessaging` behind a Windows-only API. Keep raw `HWND`/`HDWP` details inside `wm-platform`.

The abstraction should:

- Accept a collection of native windows, target rectangles, z-order values, and flags.
- Preserve the same z-order mapping currently used by `NativeWindowWindowsExt::set_window_pos`.
- Return a `Result` if beginning, appending, or committing the batch fails.
- Ensure a partially constructed defer handle is not reused after `DeferWindowPos` failure.
- Avoid `SWP_ASYNCWINDOWPOS` in the defer batch unless testing proves it is required or compatible with the intended atomic behavior.
- Keep the existing one-window `set_window_pos` method for operations that cannot be batched.

The required Windows crate feature (`Win32_UI_WindowsAndMessaging`) is already enabled in `packages/wm-platform/Cargo.toml`.

### B. Refactor redraw planning separately from execution

`packages/wm/src/commands/general/platform_sync.rs::redraw_containers` currently interleaves:

- geometry calculation;
- restore/maximize state changes;
- z-order decisions;
- visibility/cloaking;
- `SetWindowPos` calls.

Introduce a small internal planned-update structure so geometry for eligible Windows tiled windows can be collected first and submitted as one batch. Keep restore/maximize, taskbar, visibility, and fullscreen transition side effects in their existing safe order.

Initially batch only the straightforward case:

- Windows platform;
- visible, non-minimized, non-maximized tiled windows;
- windows queued for geometry redraw in the same `platform_sync` pass;
- no pending DPI double-position adjustment;
- no special `HideMethod::PlaceInCorner` operation.

Fall back to the existing individual `SetWindowPos` path for every exceptional case. Do not broaden batching until manual tests demonstrate equivalent behavior.

### C. Preserve z-order semantics

The current redraw order is derived from descendant focus order and `WindowZOrder::AfterWindow`. The batch must preserve this ordering. Verify that passing the appropriate `hWndInsertAfter` for each deferred operation produces the same final stack as the existing sequence.

Do not combine the existing delayed z-order retry in `set_z_order` with the geometry batch unless the batch actually replaces that operation.

### Phase 3 tests and acceptance criteria

- Unit-test construction/order of planned window updates without requiring real HWNDs.
- Test mixed batches where an exceptional window falls back to the individual path.
- Confirm an injected/faked batch failure reports an error and does not corrupt pending state.
- Manually resize layouts containing 2, 3, and 6+ tiled windows, including nested/dwindle splits.
- Watch specifically for transient gaps, overlaps, z-order changes, focus stealing, incorrect borders, and windows that fail to resize.
- Test mixed-DPI multi-monitor movement and resizing; preserve the existing double-position workaround for `has_pending_dpi_adjustment()`.

## Instrumentation and performance checks

For development builds, temporary tracing can record:

- emitted mouse-move interval;
- time spent in `handle_resize_move`;
- time spent in `platform_sync`;
- count of windows redrawn per resize frame;
- count of `EVENT_OBJECT_LOCATIONCHANGE` events generated during a resize.

Compare before and after over a five-second continuous resize. The desired outcome is:

- approximately 60 resize updates/s while the cursor is moving;
- no steadily growing mouse or window-event backlog;
- stable CPU usage after release;
- materially fewer full moved/resized-handler executions after Phase 2;
- one deferred geometry commit per tiled resize frame after Phase 3.

Temporary per-frame logs must not remain enabled at info level because logging itself can make the resize path stutter.

## Non-goals

- Do not add visual resize animations or interpolation. The window should follow the real cursor directly.
- Do not alter keyboard `resize`/`resizeactive` command increments.
- Do not change macOS cadence as part of this Windows fix.
- Do not redesign the tiling-size algorithm or minimum-size policy unless testing reveals a separate correctness bug.
- Do not make the interval configurable until there is evidence users need it; a named constant is sufficient initially.

## Recommended commit sequence

1. `fix: raise Windows drag update cadence and apply release position`
2. `fix: ignore self-generated location events during modifier resize`
3. `perf: batch tiled window geometry updates on Windows`

After each commit, run formatting, tests, and clippy, then manually verify resizing before continuing. If Phase 1 alone resolves the reported experience and Phase 3 introduces compatibility risk, ship Phases 1-2 first and treat batching as a follow-up optimization.
