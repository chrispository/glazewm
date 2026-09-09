use anyhow::Context;
use wm_common::TilingDirection;
use wm_platform::Direction;

use crate::{
  models::{Container, TilingContainer, TilingWindow},
  traits::{CommonGetters, TilingDirectionGetters},
};

/// Gets the nearest tiling window in the given direction.
///
/// Traverses upwards from the origin container until an ancestor with a
/// matching tiling direction has a sibling in that direction, then
/// descends into that sibling to get the window closest to the origin.
/// The search stops at the workspace, so it never crosses into another
/// workspace or monitor.
///
/// Returns `None` if there is no tiling window in the given direction
/// (e.g. the origin is against the edge of its workspace).
pub fn tiling_window_in_direction(
  origin_container: &Container,
  direction: &Direction,
) -> anyhow::Result<Option<TilingWindow>> {
  let tiling_direction = TilingDirection::from_direction(direction);
  let mut origin_or_ancestor = origin_container.clone();

  // Traverse upwards from the origin container. Stop searching when a
  // workspace is encountered.
  while !origin_or_ancestor.is_workspace() {
    let parent = origin_or_ancestor
      .parent()
      .and_then(|parent| parent.as_direction_container().ok())
      .context("No direction container.")?;

    // Skip if the tiling direction doesn't match.
    if parent.tiling_direction() != tiling_direction {
      origin_or_ancestor = parent.into();
      continue;
    }

    // Get the next/prev tiling sibling depending on the tiling direction.
    let target = match direction {
      Direction::Up | Direction::Left => origin_or_ancestor
        .prev_siblings()
        .find_map(|sibling| sibling.as_tiling_container().ok()),
      _ => origin_or_ancestor
        .next_siblings()
        .find_map(|sibling| sibling.as_tiling_container().ok()),
    };

    match target {
      // Return once a suitable target is found.
      Some(target) => {
        return Ok(match target {
          TilingContainer::TilingWindow(window) => Some(window),
          TilingContainer::Split(split) => {
            split.descendant_in_direction(&direction.inverse())
          }
        });
      }
      None => origin_or_ancestor = parent.into(),
    }
  }

  Ok(None)
}

#[cfg(test)]
mod tests {
  use wm_common::TilingDirection;
  use wm_platform::Direction;

  use super::tiling_window_in_direction;
  use crate::{
    models::{SplitContainer, TilingWindow, Workspace},
    traits::CommonGetters,
  };

  #[test]
  fn finds_the_adjacent_sibling() {
    let window_a = TilingWindow::mock().call();
    let window_b = TilingWindow::mock().call();

    let _workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![
        window_a.clone().into(),
        window_b.clone().into(),
      ])
      .call();

    let target = tiling_window_in_direction(
      &window_a.clone().into(),
      &Direction::Right,
    )
    .unwrap();

    assert_eq!(target.map(|window| window.id()), Some(window_b.id()));

    // There is nothing to the left of the first window.
    let target =
      tiling_window_in_direction(&window_a.into(), &Direction::Left)
        .unwrap();

    assert!(target.is_none());
  }

  #[test]
  fn descends_into_an_adjacent_split() {
    let window_a = TilingWindow::mock().call();
    let window_b = TilingWindow::mock().call();
    let window_c = TilingWindow::mock().call();

    let split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![window_b.clone().into(), window_c.into()])
      .call();

    let _workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![window_a.clone().into(), split.into()])
      .call();

    // The topmost window of the split is closest to the origin.
    let target =
      tiling_window_in_direction(&window_a.into(), &Direction::Right)
        .unwrap();

    assert_eq!(target.map(|window| window.id()), Some(window_b.id()));
  }

  #[test]
  fn traverses_upwards_when_the_direction_doesnt_match() {
    let window_a = TilingWindow::mock().call();
    let window_b = TilingWindow::mock().call();
    let window_c = TilingWindow::mock().call();

    let split = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![
        window_b.clone().into(),
        window_c.clone().into(),
      ])
      .call();

    let _workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![window_a.clone().into(), split.into()])
      .call();

    // A horizontal move from within the vertical split has to escape it.
    let target =
      tiling_window_in_direction(&window_c.into(), &Direction::Left)
        .unwrap();

    assert_eq!(target.map(|window| window.id()), Some(window_a.id()));
  }
}
