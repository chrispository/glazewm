use anyhow::Context;
use wm_common::{TilingDirection, WmEvent};
use wm_platform::Rect;

use super::{flatten_split_container, wrap_in_split_container};
use crate::{
  models::{
    Container, DirectionContainer, SplitContainer, TilingContainer,
    TilingWindow,
  },
  traits::{
    CommonGetters, PositionGetters, TilingDirectionGetters,
    TilingSizeGetters,
  },
  user_config::UserConfig,
  wm_state::WmState,
};

pub fn toggle_tiling_direction(
  container: Container,
  state: &mut WmState,
  _config: &UserConfig,
) -> anyhow::Result<()> {
  let direction_container = match container {
    Container::TilingWindow(tiling_window) => {
      toggle_window_direction(&tiling_window)
    }
    Container::Workspace(workspace) => {
      workspace
        .set_tiling_direction(workspace.tiling_direction().inverse());

      Ok(workspace.into())
    }
    // Can only toggle tiling direction from a tiling window or workspace.
    _ => return Ok(()),
  }?;

  // Rotating the split axis changes the geometry of every window beneath
  // the affected direction container, so queue them all for a redraw.
  state
    .pending_sync
    .queue_containers_to_redraw(direction_container.tiling_children());

  state.emit_event(WmEvent::TilingDirectionChanged {
    direction_container: direction_container.to_dto()?,
    new_tiling_direction: direction_container.tiling_direction(),
  });

  Ok(())
}

fn toggle_window_direction(
  tiling_window: &TilingWindow,
) -> anyhow::Result<DirectionContainer> {
  let parent = tiling_window
    .direction_container()
    .context("No direction container.")?;

  let tiling_siblings = tiling_window.tiling_siblings().collect::<Vec<_>>();

  // If the window is an only child, then either change the tiling
  // direction of its parent workspace or flatten its parent split
  // container.
  if tiling_siblings.is_empty() {
    return match parent {
      DirectionContainer::Workspace(workspace) => {
        workspace
          .set_tiling_direction(workspace.tiling_direction().inverse());

        Ok(workspace.into())
      }
      DirectionContainer::Split(split_container) => {
        flatten_split_container(split_container.clone())?;

        tiling_window
          .direction_container()
          .context("No direction container.")
      }
    };
  }

  // Dwindle insertion only ever creates splits that hold exactly two
  // children, so flipping the enclosing split's axis rotates precisely
  // that pair. The sibling may itself be a split container (i.e. a
  // nested subtree), which is rotated along with it — this matches
  // Hyprland's `togglesplit`.
  let parent_children = parent.tiling_children().collect::<Vec<_>>();

  if parent_children.len() == 2 {
    parent.set_tiling_direction(parent.tiling_direction().inverse());

    return Ok(parent);
  }

  // Splits with more than two children can still arise (e.g. from moving
  // windows between workspaces). Isolate the focused window and its
  // nearest neighbor into a new perpendicular split so the toggle only
  // affects the pair, and not the other children of the split.
  let position = parent_children
    .iter()
    .position(|child| child.id() == tiling_window.id())
    .context("Window is not a child of its own parent.")?;

  let Some((neighbor, neighbor_position)) =
    nearest_adjacent_sibling(tiling_window, &parent_children, position)?
  else {
    // No adjacent sibling to pair with, so fall back to flipping the
    // whole split.
    parent.set_tiling_direction(parent.tiling_direction().inverse());

    return Ok(parent);
  };

  let split_container = SplitContainer::new(
    parent.tiling_direction().inverse(),
    tiling_window.gaps_config().clone(),
  );

  // Order matters: the left/top-most container must come first so that
  // `wrap_in_split_container` preserves the visual order.
  let pair: [TilingContainer; 2] = if position < neighbor_position {
    [tiling_window.clone().into(), neighbor]
  } else {
    [neighbor, tiling_window.clone().into()]
  };

  wrap_in_split_container(&split_container, &parent.into(), &pair)?;

  Ok(split_container.into())
}

/// Returns the tiling container immediately before or after the given
/// window within its parent, whichever is closest by center-to-center
/// distance.
///
/// Candidates are restricted to adjacent siblings so that the resulting
/// pair stays contiguous, which `wrap_in_split_container` requires in
/// order to preserve the layout's visual order.
///
/// Returns the neighbor along with its position in `siblings`, or `None`
/// when the window has no siblings at all.
fn nearest_adjacent_sibling(
  tiling_window: &TilingWindow,
  siblings: &[TilingContainer],
  position: usize,
) -> anyhow::Result<Option<(TilingContainer, usize)>> {
  let window_center = rect_center(&tiling_window.to_rect()?);

  let nearest = [position.checked_sub(1), position.checked_add(1)]
    .into_iter()
    .flatten()
    .filter_map(|index| {
      siblings.get(index).map(|sibling| (sibling.clone(), index))
    })
    .min_by(|(sibling_a, _), (sibling_b, _)| {
      distance_to_center(sibling_a, window_center)
        .total_cmp(&distance_to_center(sibling_b, window_center))
    });

  Ok(nearest)
}

/// Computes the squared distance from a container's center to the given
/// point.
///
/// Returns `f32::MAX` for containers with no computable rect so that they
/// sort last.
fn distance_to_center(container: &TilingContainer, point: (f32, f32)) -> f32 {
  container.to_rect().map_or(f32::MAX, |rect| {
    square_distance(point, rect_center(&rect))
  })
}

/// Gets the center point of a rectangle.
#[must_use]
fn rect_center(rect: &Rect) -> (f32, f32) {
  #[allow(clippy::cast_precision_loss)]
  let x = (rect.x() + (rect.width() / 2)) as f32;
  #[allow(clippy::cast_precision_loss)]
  let y = (rect.y() + (rect.height() / 2)) as f32;

  (x, y)
}

/// Computes the squared Euclidean distance between two points.
fn square_distance(a: (f32, f32), b: (f32, f32)) -> f32 {
  (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

pub fn set_tiling_direction(
  container: Container,
  state: &mut WmState,
  config: &UserConfig,
  tiling_direction: &TilingDirection,
) -> anyhow::Result<()> {
  let direction_container = container
    .direction_container()
    .context("No direction container.")?;

  if direction_container.tiling_direction() == *tiling_direction {
    Ok(())
  } else {
    toggle_tiling_direction(container, state, config)
  }
}

#[cfg(test)]
mod tests {
  use wm_common::TilingDirection;

  use super::toggle_window_direction;
  use crate::{
    models::{Monitor, SplitContainer, TilingWindow, Workspace},
    traits::{CommonGetters, TilingDirectionGetters},
  };

  /// Toggling a window whose only sibling is a nested split must rotate
  /// the enclosing split, rather than failing to find a window sibling.
  #[test]
  fn toggles_split_with_nested_sibling() {
    let window = TilingWindow::mock().tiling_size(0.5).call();

    let nested = SplitContainer::mock()
      .tiling_direction(TilingDirection::Vertical)
      .tiling_containers(vec![
        TilingWindow::mock().call().into(),
        TilingWindow::mock().call().into(),
      ])
      .call();

    let parent = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![window.clone().into(), nested.into()])
      .call();

    let _workspace = Workspace::mock()
      .tiling_containers(vec![parent.clone().into()])
      .call();

    let result = toggle_window_direction(&window).unwrap();

    assert_eq!(parent.tiling_direction(), TilingDirection::Vertical);
    assert_eq!(result.id(), parent.id());
  }

  #[test]
  fn toggles_split_of_a_window_pair() {
    let window = TilingWindow::mock().tiling_size(0.5).call();

    let parent = SplitContainer::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![
        window.clone().into(),
        TilingWindow::mock().tiling_size(0.5).call().into(),
      ])
      .call();

    let _workspace = Workspace::mock()
      .tiling_containers(vec![parent.clone().into()])
      .call();

    toggle_window_direction(&window).unwrap();

    assert_eq!(parent.tiling_direction(), TilingDirection::Vertical);
  }

  #[test]
  fn toggles_workspace_direction_for_an_only_child() {
    let window = TilingWindow::mock().call();

    let workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![window.clone().into()])
      .call();

    let result = toggle_window_direction(&window).unwrap();

    assert_eq!(workspace.tiling_direction(), TilingDirection::Vertical);
    assert_eq!(result.id(), workspace.id());
  }

  /// Splits with more than two children isolate the focused window and an
  /// adjacent sibling into a new perpendicular split.
  #[test]
  fn isolates_a_pair_out_of_a_larger_split() {
    let window = TilingWindow::mock().tiling_size(1.0 / 3.0).call();

    let workspace = Workspace::mock()
      .tiling_direction(TilingDirection::Horizontal)
      .tiling_containers(vec![
        TilingWindow::mock().tiling_size(1.0 / 3.0).call().into(),
        window.clone().into(),
        TilingWindow::mock().tiling_size(1.0 / 3.0).call().into(),
      ])
      .call();

    // A monitor is needed so that the containers have computable rects.
    let _monitor = Monitor::mock().workspaces(vec![workspace.clone()]).call();

    let result = toggle_window_direction(&window).unwrap();

    assert_eq!(result.tiling_direction(), TilingDirection::Vertical);
    assert_eq!(result.tiling_children().count(), 2);

    // The workspace keeps its direction and is left with the new split
    // plus the remaining window.
    assert_eq!(workspace.tiling_direction(), TilingDirection::Horizontal);
    assert_eq!(workspace.tiling_children().count(), 2);

    let new_parent = window.parent().unwrap();
    assert_eq!(new_parent.id(), result.id());
  }
}
