use std::sync::{
  atomic::{AtomicBool, Ordering},
  Arc,
};

use crate::{platform_event::MouseButton, platform_impl, Dispatcher};

/// A mouse button event received from [`MouseHook`].
#[derive(Clone, Debug)]
pub struct MouseButtonEvent {
  /// The button that was pressed or released.
  pub button: MouseButton,

  /// Whether the event is for a button press or release.
  pub is_press: bool,
}

/// A system-wide low-level mouse hook for intercepting button events.
///
/// Unlike [`MouseListener`](crate::MouseListener), which observes mouse
/// input without affecting it, this hook can swallow button events so
/// that they never reach the application under the cursor. It is used for
/// mouse bindings (e.g. Win+right-drag), where the click would otherwise
/// also be handled by the application.
///
/// # Platform-specific
///
/// - **Windows**: Backed by a `WH_MOUSE_LL` hook that runs on the
///   dispatcher's thread. The callback must return quickly, otherwise
///   Windows silently drops the hook.
#[derive(Debug)]
pub struct MouseHook {
  /// Whether the hook's callback is consulted for incoming events.
  is_enabled: Arc<AtomicBool>,

  /// Inner platform-specific mouse hook.
  inner: platform_impl::MouseHook,
}

impl MouseHook {
  /// Creates a new [`MouseHook`].
  ///
  /// The callback is called for every mouse button event and returns
  /// `true` if the event should be intercepted. Intercepted events are
  /// not delivered to any other application.
  pub fn new<F>(callback: F, dispatcher: &Dispatcher) -> crate::Result<Self>
  where
    F: Fn(&MouseButtonEvent) -> bool + Send + Sync + 'static,
  {
    let is_enabled = Arc::new(AtomicBool::new(true));

    let inner = {
      let is_enabled = Arc::clone(&is_enabled);

      platform_impl::MouseHook::new(
        move |event| {
          is_enabled.load(Ordering::Relaxed) && callback(event)
        },
        dispatcher,
      )?
    };

    Ok(Self { is_enabled, inner })
  }

  /// Enables or disables the hook.
  ///
  /// While disabled, the hook remains registered but no events are
  /// intercepted.
  pub fn enable(&mut self, enabled: bool) {
    self.is_enabled.store(enabled, Ordering::Relaxed);
  }

  /// Terminates the mouse hook by unregistering it.
  pub fn terminate(&mut self) -> crate::Result<()> {
    self.inner.terminate()
  }
}
