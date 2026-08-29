use anyhow::Context;
use wm_common::TilingDirection;

use super::{attach_container, wrap_in_split_container};
use crate::{
  models::{Container, SplitContainer, TilingWindow},
  traits::{CommonGetters, PositionGetters, TilingSizeGetters},
};

/// Gets the tiling window that a newly managed window will split, if
/// any.
///
/// Rules (mirroring Hyprland's dwindle behavior):
/// - If a tiling window is focused, it becomes the split anchor.
/// - Otherwise, the first tiling window in descendant focus order of the
///   focused workspace is used.
/// - Returns `None` when the workspace has no tiling windows yet.
pub fn dwindle_split_target(
  state: &crate::wm_state::WmState,
) -> anyhow::Result<Option<TilingWindow>> {
  let focused_container =
    state.focused_container().context("No focused container.")?;

  let focused_workspace =
    focused_container.workspace().context("No workspace.")?;

  let anchor_candidate = if focused_container.is_tiling_window() {
    Some(focused_container)
  } else {
    focused_workspace
      .descendant_focus_order()
      .find(Container::is_tiling_window)
  };

  Ok(match anchor_candidate {
    Some(Container::TilingWindow(tiling_window)) => Some(tiling_window),
    _ => None,
  })
}

/// Inserts a new tiling window by splitting an existing window's area,
/// mimicking Hyprland's dwindle layout.
///
/// The existing window (`anchor`) is wrapped in a new split container
/// together with `new_window`, so both windows end up sharing half of
/// the anchor's previous area each. The split direction is chosen based
/// on the aspect ratio of the anchor: wide anchors are split side-by-side
/// (horizontal), tall anchors are stacked (vertical).
pub fn dwindle_insert(
  anchor: &TilingWindow,
  new_window: &Container,
) -> anyhow::Result<()> {
  let rect = anchor.to_rect()?;
  let gaps_config = anchor.gaps_config().clone();

  // Hyprland's dwindle rule: prefer horizontal splitting (side-by-side)
  // unless the available space is taller than it is wide.
  let tiling_direction = if rect.width() >= rect.height() {
    TilingDirection::Horizontal
  } else {
    TilingDirection::Vertical
  };

  let split_container = SplitContainer::new(tiling_direction, gaps_config);

  let parent = anchor.parent().context("No parent.")?;

  wrap_in_split_container(&split_container, &parent, &[anchor.clone().into()])?;

  // Insert the new window as the sibling of the anchor within the new
  // split container. The attach resizes both windows to share the
  // anchor's previous area equally.
  attach_container(new_window, &split_container.clone().into(), Some(1))?;

  Ok(())
}
