use anyhow::Context;
#[cfg(target_os = "macos")]
use wm_common::try_warn;
use wm_common::{
  ActiveDrag, ActiveDragOperation, FloatingStateConfig, WindowState,
};
#[cfg(target_os = "windows")]
use wm_platform::{SWP_NOACTIVATE, SWP_NOSENDCHANGING};
use wm_platform::{
  Key, MouseButton, MouseEvent, Point, Rect, WindowId, WindowZOrder,
};
#[cfg(target_os = "windows")]
use wm_platform::NativeWindowWindowsExt;

use crate::{
  commands::{
    container::set_focused_descendant,
    window::update_window_state,
  },
  traits::{CommonGetters, WindowGetters},
  user_config::UserConfig,
  wm_state::WmState,
};
#[cfg(target_os = "macos")]
use crate::{
  events::handle_window_moved_or_resized_end, traits::WindowGetters,
};

pub fn handle_mouse_move(
  event: &MouseEvent,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  // Ignore mouse move events if the WM is paused. The mouse listener
  // should anyways be disabled when the WM is paused, but this is just in
  // case any events slipped through while disabling.
  if state.is_paused {
    return Ok(());
  }

  // On macOS, detect when a window drag operation has ended by listening
  // to the release of left click.
  //
  // This cannot be used for Windows, since it leads to race conditions
  // where the mouse event comes in before the `MovedOrResized` event with
  // `is_interactive_end`. For example, if the user drags to maximize a
  // window, the WS_MAXIMIZED state is sometimes set after the mouse event.
  #[cfg(target_os = "macos")]
  if let MouseEvent::ButtonUp { button, .. } = event {
    if *button == MouseButton::Left {
      let active_drag_windows = state
        .windows()
        .into_iter()
        .filter(|window| window.active_drag().is_some());

      // Only one window should ever be actively dragged at a time, but
      // just in case, iterate over all active drag windows.
      for window in active_drag_windows {
        let new_rect = try_warn!(window.native().frame());

        window.update_native_properties(|properties| {
          properties.frame = new_rect;
        });

        handle_window_moved_or_resized_end(&window, state, config)?;
      }
    }

    return Ok(());
  }

  match event {
    MouseEvent::ButtonDown { button, position, .. }
      if *button == MouseButton::Left =>
    {
      handle_drag_start(position, state, config)?;
    }
    MouseEvent::Move {
      position,
      pressed_buttons,
      // LINT: `window_below_cursor` is only used on macOS.
      #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
      window_below_cursor,
      ..
    } => {
      // If a modifier-drag is active, move the dragged window with the
      // cursor regardless of focus-follows-cursor.
      if state.drag_move.is_some() {
        handle_drag_move(position, state, config)?;
        return Ok(());
      }

      // Ignore event if left/right-click is down. Otherwise, this causes
      // focus to jitter when a window is being resized by its drag
      // handles. Also ignore if the OS focused window isn't the same as
      // the WM's focused window.
      if pressed_buttons.contains(&MouseButton::Left)
        || pressed_buttons.contains(&MouseButton::Right)
        || !state.is_focus_synced
        || !config.value.general.focus_follows_cursor
      {
        return Ok(());
      }

      #[cfg(target_os = "macos")]
      let window_under_cursor = window_below_cursor.and_then(|window_id| {
        use crate::traits::WindowGetters;

        state
          .windows()
          .into_iter()
          .find(|w| w.native().id() == window_id)
      });

      #[cfg(target_os = "macos")]
      focus_window_or_monitor(position, window_under_cursor, state)?;

      #[cfg(target_os = "windows")]
      focus_window_or_monitor(position, None, state)?;
    }
    MouseEvent::ButtonUp { button, .. }
      if state.drag_move.is_some() && *button == MouseButton::Left =>
    {
      handle_drag_end(state, config)?;
    }
    _ => {}
  }

  Ok(())
}

/// Starts a modifier-drag (Win+drag) on the tiling window under the
/// cursor.
///
/// The window is converted to a temporary floating window (the "float
/// preview") so it can follow the cursor, and a [`DragMove`] is recorded
/// for the duration of the drag. On release, the window is dropped back
/// into the tiling layout.
fn handle_drag_start(
  position: &Point,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  // Only trigger on Windows, and only when the Win (super) key is held.
  #[cfg(not(target_os = "windows"))]
  {
    let _ = (position, state, config);
    return Ok(());
  }
  #[cfg(target_os = "windows")]
  {
    if !state.dispatcher.is_key_down(Key::Win) {
      return Ok(());
    }

    // Find the tiling window under the cursor.
    let Some(native_window) = state
      .dispatcher
      .window_from_point(position)?
      .and_then(|native| state.window_from_native(&native))
    else {
      return Ok(());
    };

    if !matches!(native_window.state(), WindowState::Tiling) {
      return Ok(());
    }

    let frame = native_window.native_properties().frame;

    // Convert the tiling window to a temporary floating window so it can
    // be freely repositioned. `is_from_floating` is set to `false` so
    // that the existing drop-as-tiling logic re-tiles it on release.
    let floating_state = FloatingStateConfig {
      centered: false,
      ..config.value.window_behavior.state_defaults.floating
    };

    let window = update_window_state(
      native_window.clone(),
      WindowState::Floating(floating_state),
      state,
      config,
    )?;

    window.set_active_drag(Some(ActiveDrag {
      operation: Some(ActiveDragOperation::Move),
      is_from_floating: false,
      initial_position: frame.clone(),
    }));

    // Record the grab offset so the window tracks the cursor exactly.
    state.drag_move = Some(crate::wm_state::DragMove {
      window_id: window.id(),
      grab_offset: Point {
        x: position.x - frame.x(),
        y: position.y - frame.y(),
      },
    });

    Ok(())
  }
}

/// Moves the actively-dragged window so its top-left stays at
/// `position - grab_offset`.
fn handle_drag_move(
  position: &Point,
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let drag_move = state
    .drag_move
    .as_ref()
    .context("No active drag.")?
    .clone();

  let window = state
    .window_by_id(drag_move.window_id)
    .context("Dragged window no longer exists.")?;

  let frame = window.native_properties().frame;

  let target_x = position.x - drag_move.grab_offset.x;
  let target_y = position.y - drag_move.grab_offset.y;

  // Clamp to the work area of the monitor under the cursor so the
  // window can't be dragged off-screen.
  let target_rect = if let Some(monitor) = state.monitor_at_point(position) {
    let work_area = monitor.native_properties().working_area;
    let rect =
      Rect::from_xy(target_x, target_y, frame.width(), frame.height());
    clamp_rect_to_area(&rect, &work_area)
  } else {
    Rect::from_xy(target_x, target_y, frame.width(), frame.height())
  };

  #[cfg(target_os = "windows")]
  {
    window
      .native()
      .set_window_pos(
        &WindowZOrder::Normal,
        &target_rect,
        SWP_NOACTIVATE | SWP_NOSENDCHANGING,
      )
      .context("Failed to move dragged window.")?;
  }

  // Update the cached native frame so subsequent logic uses the new
  // position.
  window.update_native_properties(|properties| {
    properties.frame = target_rect;
  });

  let _ = config;
  Ok(())
}

/// Ends an active modifier-drag, dropping the window back into the tiling
/// layout at the cursor's location.
fn handle_drag_end(
  state: &mut WmState,
  config: &UserConfig,
) -> anyhow::Result<()> {
  let drag_move = state
    .drag_move
    .as_ref()
    .context("No active drag.")?
    .clone();

  let window = state
    .window_by_id(drag_move.window_id)
    .context("Dragged window no longer exists.")?;

  state.drag_move = None;

  #[cfg(target_os = "windows")]
  {
    // On Windows the drag lifecycle is fully driven by mouse events, so
    // end the drag through the existing drop-as-tiling logic.
    crate::events::handle_window_moved_or_resized_end(&window, state, config)?;
  }
  #[cfg(not(target_os = "windows"))]
  {
    let _ = (window, config);
  }

  Ok(())
}

/// Focuses the window (or monitor) under the cursor when
/// `focus_follows_cursor` is enabled.
///
/// On macOS the window under the cursor is passed in from the event
/// payload; on Windows it must be looked up via the dispatcher.
fn focus_window_or_monitor(
  position: &Point,
  window_under_cursor: Option<WindowId>,
  state: &mut WmState,
) -> anyhow::Result<()> {
  #[cfg(target_os = "windows")]
  let window_under_cursor = window_under_cursor.or_else(|| {
    state
      .dispatcher
      .window_from_point(position)
      .ok()
      .flatten()
      .and_then(|native| state.window_from_native(&native))
      .map(|window| window.native().id())
  });

  // Set focus to whichever window is currently under the cursor.
  if let Some(window_id) = window_under_cursor {
    let window = state
      .windows()
      .into_iter()
      .find(|window| window.native().id() == window_id);

    if let Some(window) = window {
      let focused_container =
        state.focused_container().context("No focused container.")?;

      if focused_container.id() != window.id() {
        set_focused_descendant(&window.as_container(), None);
        state.pending_sync.queue_focus_change();
      }
    }
  } else {
    // Focus the monitor if no window is under the cursor.
    let cursor_monitor = state
      .monitor_at_point(position)
      .context("No monitor under cursor.")?;

    let focused_monitor = state
      .focused_container()
      .context("No focused container.")?
      .monitor()
      .context("Focused container has no monitor.")?;

    // Avoid setting focus to the same monitor.
    if cursor_monitor.id() != focused_monitor.id() {
      set_focused_descendant(&cursor_monitor.as_container(), None);
      state.pending_sync.queue_focus_change();
    }
  }

  Ok(())
}

/// Clamps a rectangle so it stays fully within the given area, keeping
/// its size.
///
/// If the rectangle is larger than the area, its top-left is clamped to
/// the area's top-left.
#[must_use]
fn clamp_rect_to_area(rect: &Rect, area: &Rect) -> Rect {
  let width = rect.width().min(area.width());
  let height = rect.height().min(area.height());

  let x = rect.x().clamp(area.x(), area.right - width);
  let y = rect.y().clamp(area.y(), area.bottom - height);

  Rect::from_xy(x, y, width, height)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn clamps_rect_within_area() {
    let area = Rect::from_xy(100, 100, 1920, 1080);
    // Window dragged far to the right/bottom.
    let rect = Rect::from_xy(5000, 5000, 800, 600);

    let clamped = clamp_rect_to_area(&rect, &area);

    // Right/bottom edge should align with the area.
    assert_eq!(clamped.right, 100 + 1920);
    assert_eq!(clamped.bottom, 100 + 1080);
  }

  #[test]
  fn clamps_rect_to_negative_edge() {
    let area = Rect::from_xy(100, 100, 1920, 1080);
    // Window dragged far off the top-left.
    let rect = Rect::from_xy(-5000, -5000, 800, 600);

    let clamped = clamp_rect_to_area(&rect, &area);

    // Top-left edge should align with the area.
    assert_eq!(clamped.x(), 100);
    assert_eq!(clamped.y(), 100);
  }

  #[test]
  fn keeps_rect_unchanged_when_inside() {
    let area = Rect::from_xy(100, 100, 1920, 1080);
    let rect = Rect::from_xy(200, 200, 800, 600);

    let clamped = clamp_rect_to_area(&rect, &area);

    assert_eq!(clamped, rect);
  }

  #[test]
  fn shrinks_rect_if_larger_than_area() {
    let area = Rect::from_xy(0, 0, 1000, 1000);
    let rect = Rect::from_xy(-50, -50, 3000, 3000);

    let clamped = clamp_rect_to_area(&rect, &area);

    assert_eq!(clamped.width(), 1000);
    assert_eq!(clamped.height(), 1000);
    assert_eq!(clamped.x(), 0);
    assert_eq!(clamped.y(), 0);
  }
}
