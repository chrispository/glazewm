use std::cell::Cell;

use windows::Win32::{
  Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM},
  UI::WindowsAndMessaging::{
    CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK,
    MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_RBUTTONDOWN, WM_RBUTTONUP,
  },
};

use super::FOREGROUND_INPUT_IDENTIFIER;
use crate::{Dispatcher, MouseButton, MouseButtonEvent};

/// Callback stored in [`HOOK`] for intercepting mouse button events.
type HookCallback = Box<dyn Fn(&MouseButtonEvent) -> bool>;

thread_local! {
  /// Stores the hook callback for the current thread.
  ///
  /// The hook callback is called for every mouse button event and returns
  /// `true` if the event should be intercepted.
  static HOOK: Cell<Option<HookCallback>> = Cell::default();
}

/// Platform-specific implementation of [`MouseHook`].
#[derive(Debug)]
pub(crate) struct MouseHook {
  handle: HHOOK,
  dispatcher: Dispatcher,
}

impl MouseHook {
  /// Implements [`MouseHook::new`].
  ///
  /// # Panics
  ///
  /// Panics when attempting to register multiple hooks on the
  /// dispatcher's thread.
  pub(crate) fn new<F>(
    callback: F,
    dispatcher: &Dispatcher,
  ) -> crate::Result<Self>
  where
    F: Fn(&MouseButtonEvent) -> bool + Send + Sync + 'static,
  {
    let handle = dispatcher.dispatch_sync(move || {
      HOOK.with(|state| {
        assert!(
          state.take().is_none(),
          "Only one mouse hook can be registered on the dispatcher's thread."
        );

        state.set(Some(Box::new(callback)));
      });

      unsafe {
        SetWindowsHookExW(
          WH_MOUSE_LL,
          Some(Self::hook_proc),
          HINSTANCE::default(),
          0,
        )
      }
    })??;

    Ok(Self {
      handle,
      dispatcher: dispatcher.clone(),
    })
  }

  /// Implements [`MouseHook::terminate`].
  pub(crate) fn terminate(&mut self) -> crate::Result<()> {
    unsafe { UnhookWindowsHookEx(self.handle) }?;

    // Dispatch cleanup to the event loop thread since the callback is
    // stored in a thread-local on that thread.
    let _ = self.dispatcher.dispatch_async(|| {
      HOOK.with(|state| {
        state.take();
      });
    });

    Ok(())
  }

  /// Hook procedure for mouse events.
  ///
  /// For use with `SetWindowsHookExW`.
  extern "system" fn hook_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
  ) -> LRESULT {
    // If the code is less than zero, the hook procedure must pass the hook
    // notification directly to other applications.
    if code != 0 {
      return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    #[allow(clippy::cast_possible_truncation)]
    let (button, is_press) = match wparam.0 as u32 {
      WM_LBUTTONDOWN => (MouseButton::Left, true),
      WM_LBUTTONUP => (MouseButton::Left, false),
      WM_RBUTTONDOWN => (MouseButton::Right, true),
      WM_RBUTTONUP => (MouseButton::Right, false),
      // Mouse moves and wheel events are never intercepted.
      _ => return unsafe { CallNextHookEx(None, code, wparam, lparam) },
    };

    // SAFETY: `lparam` points to a `MSLLHOOKSTRUCT` for `WH_MOUSE_LL`.
    let input = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };

    // Pass through input that this process synthesized (see
    // `NativeWindow::focus`).
    #[allow(clippy::cast_possible_truncation)]
    if input.dwExtraInfo as u32 == FOREGROUND_INPUT_IDENTIFIER {
      return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let event = MouseButtonEvent { button, is_press };

    let should_intercept = HOOK.with(|state| {
      if let Some(callback) = state.take() {
        let result = callback(&event);
        state.set(Some(callback));
        result
      } else {
        false
      }
    });

    if should_intercept {
      return LRESULT(1);
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
  }
}

impl Drop for MouseHook {
  fn drop(&mut self) {
    let _ = self.terminate();
  }
}
