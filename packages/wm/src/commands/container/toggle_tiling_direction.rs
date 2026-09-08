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

  // When the enclosing split contains exactly the focused window and one
  // sibling window (i.e. a plain pair), changing its axis rotates just
  // that pair.
  let parent_children = parent.tiling_children().collect::<Vec<_>>();
  let is_plain_pair = parent_children.len() == 2
    && parent_children
      .iter()
      .all(|child| matches!(child, TilingContainer::TilingWindow(_)));

  if is_plain_pair {
    parent.set_tiling_direction(parent.tiling_direction().inverse());

    return Ok(parent);
  }

  // Otherwise, isolate the focused window and its nearest neighbor into
  // a new perpendicular split so the toggle only affects the pair (and
  // not the other windows in the split).
  let nearest_sibling = nearest_sibling(tiling_window, &tiling_siblings)?;

  let split_container = SplitContainer::new(
    parent.tiling_direction().inverse(),
    tiling_window.gaps_config().clone(),
  );

  // Order matters: the focused window must come first so the near
  // sibling is placed on the opposite side.
  let pair = nearest_sibling_window(tiling_window, &nearest_sibling);

  wrap_in_split_container(
    &split_container,
    &parent.into(),
    &[pair.0, pair.1],
  )?;

  Ok(split_container.into())
}

/// Returns the closest tiling sibling to the given window, based on the
/// center-to-center distance of their rectangles.
fn nearest_sibling(
  tiling_window: &TilingWindow,
  siblings: &[TilingContainer],
) -> anyhow::Result<TilingWindow> {
  let window_rect = tiling_window.to_rect()?;
  let window_center = rect_center(&window_rect);

  siblings
    .iter()
    .filter_map(|sibling| match sibling {
      TilingContainer::TilingWindow(window) => Some(window.clone()),
      TilingContainer::Split(_) => None,
    })
    .min_by_key(|sibling| {
      let distance = sibling
        .to_rect()
        .map_or(f32::MAX, |r| {
          let center = rect_center(&r);
          square_distance(window_center, center)
        });

      // Use an integer key so `min_by_key` can be used directly.
      distance.to_bits()
    })
    .context("Unable to find nearest sibling.")
}

/// Returns the focused window and its neighbor as a pair of
/// [`TilingContainer`]s in left-to-right / top-to-bottom order so that
/// `wrap_in_split_container` preserves the visual order.
#[must_use]
fn nearest_sibling_window(
  tiling_window: &TilingWindow,
  nearest: &TilingWindow,
) -> (TilingContainer, TilingContainer) {
  let focused = tiling_window.clone().into();
  let neighbor = nearest.clone().into();

  // Ensure the pair is ordered by their position in the parent so that
  // the left/top-most window stays on the left/top.
  if tiling_window.index() < nearest.index() {
    (focused, neighbor)
  } else {
    (neighbor, focused)
  }
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
