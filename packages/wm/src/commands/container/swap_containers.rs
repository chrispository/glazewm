use anyhow::{bail, Context};
use uuid::Uuid;

use super::set_focused_descendant;
use crate::{
  models::Container,
  traits::{CommonGetters, TilingSizeGetters},
};

/// Swaps the position of two attached containers within the tree.
///
/// Each container takes over the other's slot: its index amongst its new
/// siblings, its place in the parent's focus order, and (if both are
/// tiling containers) its tiling size. The layout is therefore left
/// untouched apart from the two containers trading places.
///
/// Focus is preserved on whichever of the two containers was focused.
///
/// Errors if the containers are the same, if either is detached, or if
/// one is an ancestor of the other (which would detach a subtree).
pub fn swap_containers(
  container_a: &Container,
  container_b: &Container,
) -> anyhow::Result<()> {
  if container_a == container_b {
    bail!("Cannot swap a container with itself.");
  }

  if container_a
    .self_and_ancestors()
    .any(|ancestor| ancestor == *container_b)
    || container_b
      .self_and_ancestors()
      .any(|ancestor| ancestor == *container_a)
  {
    bail!("Cannot swap a container with one of its ancestors.");
  }

  let parent_a = container_a.parent().context("No parent.")?;
  let parent_b = container_b.parent().context("No parent.")?;

  // Look the indexes up explicitly, so that the assignments below are
  // known to be in bounds.
  let index_a = child_index(&parent_a, container_a)?;
  let index_b = child_index(&parent_b, container_b)?;

  // Whether either container is the currently focused container. Has to
  // be read before the swap, since it walks the containers' ancestors.
  let has_focus_a = container_a.has_focus(None);
  let has_focus_b = container_b.has_focus(None);

  if parent_a == parent_b {
    parent_a.borrow_children_mut().swap(index_a, index_b);
  } else {
    parent_a.borrow_children_mut()[index_a] = container_b.clone();
    parent_b.borrow_children_mut()[index_b] = container_a.clone();

    *container_a.borrow_parent_mut() = Some(parent_b.clone());
    *container_b.borrow_parent_mut() = Some(parent_a.clone());

    // Each container inherits the other's place in the focus order, so
    // that the focus recency of the slot is retained.
    replace_in_focus_order(&parent_a, container_a.id(), container_b.id());
    replace_in_focus_order(&parent_b, container_b.id(), container_a.id());
  }

  // Tiling sizes belong to the slot rather than to the container, so
  // that a swap doesn't change the layout's proportions.
  if let (Ok(tiling_a), Ok(tiling_b)) = (
    container_a.as_tiling_container(),
    container_b.as_tiling_container(),
  ) {
    let tiling_size_a = tiling_a.tiling_size();
    tiling_a.set_tiling_size(tiling_b.tiling_size());
    tiling_b.set_tiling_size(tiling_size_a);
  }

  // Restore the focus chain of the focused container, since its
  // ancestors have changed.
  if has_focus_a {
    set_focused_descendant(container_a, None);
  } else if has_focus_b {
    set_focused_descendant(container_b, None);
  }

  Ok(())
}

/// Gets the index of a container amongst its parent's children.
fn child_index(
  parent: &Container,
  child: &Container,
) -> anyhow::Result<usize> {
  parent
    .borrow_children()
    .iter()
    .position(|sibling| sibling.id() == child.id())
    .context("Container is not a child of its parent.")
}

/// Replaces an ID in a parent's child focus order, keeping its position.
///
/// No-op if the ID isn't in the focus order.
fn replace_in_focus_order(parent: &Container, old_id: Uuid, new_id: Uuid) {
  let mut focus_order = parent.borrow_child_focus_order_mut();

  if let Some(index) = focus_order.iter().position(|id| *id == old_id) {
    focus_order[index] = new_id;
  }
}

#[cfg(test)]
mod tests {
  use wm_common::TilingDirection;

  use super::{set_focused_descendant, swap_containers};
  use crate::{
    models::{SplitContainer, TilingWindow, Workspace},
    traits::{CommonGetters, TilingSizeGetters},
  };

  #[test]
  fn swaps_siblings_and_keeps_slot_sizes() {
    let window_a = TilingWindow::mock().call();
    let window_b = TilingWindow::mock().call();

    let workspace = Workspace::mock()
      .tiling_containers(vec![
        window_a.clone().into(),
        window_b.clone().into(),
      ])
      .call();

    window_a.set_tiling_size(0.7);
    window_b.set_tiling_size(0.3);

    swap_containers(&window_a.clone().into(), &window_b.clone().into())
      .unwrap();

    let children = workspace.children();
    assert_eq!(children[0].id(), window_b.id());
    assert_eq!(children[1].id(), window_a.id());

    // The first slot should still take up 70% of the workspace.
    assert!((window_b.tiling_size() - 0.7).abs() < f32::EPSILON);
    assert!((window_a.tiling_size() - 0.3).abs() < f32::EPSILON);
  }

  #[test]
  fn swaps_containers_across_parents() {
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

    let workspace = Workspace::mock()
      .tiling_containers(vec![
        window_a.clone().into(),
        split.clone().into(),
      ])
      .call();

    window_a.set_tiling_size(0.6);
    window_b.set_tiling_size(0.4);

    swap_containers(&window_a.clone().into(), &window_b.clone().into())
      .unwrap();

    assert_eq!(workspace.children()[0].id(), window_b.id());
    assert_eq!(split.children()[0].id(), window_a.id());
    assert_eq!(window_a.parent().unwrap().id(), split.id());
    assert_eq!(window_b.parent().unwrap().id(), workspace.id());

    // Sizes stay with the slot, not with the window.
    assert!((window_b.tiling_size() - 0.6).abs() < f32::EPSILON);
    assert!((window_a.tiling_size() - 0.4).abs() < f32::EPSILON);

    // The untouched sibling should keep its position.
    assert_eq!(split.children()[1].id(), window_c.id());
  }

  #[test]
  fn preserves_focus_of_the_focused_container() {
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
      .tiling_containers(vec![window_a.clone().into(), split.into()])
      .call();

    set_focused_descendant(&window_a.clone().into(), None);
    assert!(window_a.has_focus(None));

    swap_containers(&window_a.clone().into(), &window_b.clone().into())
      .unwrap();

    // Focus should follow the window into its new parent.
    assert!(window_a.has_focus(None));
    assert!(!window_b.has_focus(None));
  }

  #[test]
  fn rejects_swapping_a_container_with_its_ancestor() {
    let window_a = TilingWindow::mock().call();

    let split = SplitContainer::mock()
      .tiling_containers(vec![
        window_a.clone().into(),
        TilingWindow::mock().call().into(),
      ])
      .call();

    let _workspace = Workspace::mock()
      .tiling_containers(vec![split.clone().into()])
      .call();

    assert!(
      swap_containers(&window_a.clone().into(), &split.into()).is_err()
    );
    assert!(
      swap_containers(&window_a.clone().into(), &window_a.into()).is_err()
    );
  }
}
