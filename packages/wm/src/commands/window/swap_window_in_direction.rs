use wm_common::WmEvent;
use wm_platform::Direction;

use crate::{
  commands::container::{swap_containers, tiling_window_in_direction},
  models::WindowContainer,
  traits::CommonGetters,
  wm_state::WmState,
};

/// Swaps a tiling window with the nearest tiling window in the given
/// direction.
///
/// Both windows take over the size of the slot they move into, so the
/// layout is left intact and only the two windows trade places. Focus
/// stays with the window that was focused, which means that repeated
/// swaps carry it across the layout.
///
/// No-op if the window isn't tiling, or if there is no tiling window in
/// the given direction within its workspace. Unlike `move_window`, this
/// never moves the window to another workspace or monitor.
pub fn swap_window_in_direction(
  window: &WindowContainer,
  direction: &Direction,
  state: &mut WmState,
) -> anyhow::Result<()> {
  let WindowContainer::TilingWindow(window) = window else {
    return Ok(());
  };

  let Some(target_window) =
    tiling_window_in_direction(&window.clone().into(), direction)?
  else {
    return Ok(());
  };

  swap_containers(&window.clone().into(), &target_window.clone().into())?;

  // At most one of the two windows can have focus, but either of them
  // might, since the swap isn't necessarily aimed at the focused window.
  let focused_window = [window.clone(), target_window.clone()]
    .into_iter()
    .find(|window| window.has_focus(None));

  state
    .pending_sync
    .queue_container_to_redraw(window.clone())
    .queue_container_to_redraw(target_window);

  if let Some(focused_window) = focused_window {
    state.emit_event(WmEvent::FocusedContainerMoved {
      focused_container: focused_window.to_dto()?,
    });
  }

  Ok(())
}
