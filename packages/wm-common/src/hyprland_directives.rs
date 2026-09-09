use tracing::{debug, info, warn};
use wm_platform::{Direction, Key, Keybinding, LengthValue};

use crate::{
  app_command::{InvokeCommand, InvokeFocusCommand, InvokeMoveCommand, InvokeResizeCommand},
  KeybindingConfig,
};

/// Extracted Hyprland-style directives found in the user config.
pub struct HyprlandDirectives {
  /// Keybindings translated from `bind = ...` lines.
  pub keybindings: Vec<KeybindingConfig>,

  /// Commands translated from `exec-once = ...` lines.
  pub startup_commands: Vec<InvokeCommand>,

  /// Commands translated from `exec-shutdown = ...` lines.
  pub shutdown_commands: Vec<InvokeCommand>,
}

/// Classification of a Hyprland-style directive line.
#[derive(PartialEq)]
enum DirectiveKind {
  /// `bind`, `binde`, `bindm`, etc.
  Bind,

  /// `exec-once` / `execr-once`.
  ExecOnce,

  /// `exec-shutdown` / `execr-shutdown`.
  ExecShutdown,

  /// `submap` — unsupported scoping mechanism; stripped and warned about,
  /// with subsequent binds kept global.
  Submap,

  /// Recognized Hyprland directives that have no equivalent (e.g.
  /// `windowrulev2`); stripped and warned about.
  StrippedUnsupported,
}

/// Scans a user config string for Hyprland-style directives (e.g.
/// `bind = SUPER, Q, killactive` and `exec-once = firefox`).
///
/// Directive lines are stripped from the returned config string so that
/// the remainder can be parsed as regular YAML, and matching directives
/// are translated into their `GlazeWM` equivalents.
///
/// Supported directives:
/// - `bind*` (e.g. `bind`, `binde`) — all treated identically; mouse
///   bindings (`bindm`) are reported as unsupported.
/// - `exec-once` / `execr-once` — mapped to WM startup commands.
/// - `exec-shutdown` / `execr-shutdown` — mapped to WM shutdown commands.
///
/// Limitations:
/// - `submap` sections are stripped; their bindings remain global.
/// - Directives without an equivalent (e.g. `windowrulev2`, `env`) are
///   stripped and logged so pasting chunks of a `hyprland.conf` does not
///   break YAML parsing.
///
/// Returns the stripped config string along with the extracted
/// [`HyprlandDirectives`].
#[must_use]
pub fn extract_hyprland_directives(
  config_str: &str,
) -> (String, HyprlandDirectives) {
  let mut stripped_lines: Vec<String> =
    Vec::with_capacity(config_str.lines().count());
  let mut keybindings: Vec<KeybindingConfig> = Vec::new();
  let mut startup_commands: Vec<InvokeCommand> = Vec::new();
  let mut shutdown_commands: Vec<InvokeCommand> = Vec::new();

  for (line_num, line) in config_str.lines().enumerate() {
    let line_number = line_num + 1;

    let Some((directive_name, value)) = parse_directive_line(line) else {
      stripped_lines.push(line.to_string());
      continue;
    };

    let kind = classify_directive(directive_name);

    match kind {
      DirectiveKind::Bind => {
        match translate_bind(&value) {
          Ok(binding) => {
            debug!("Translated Hyprland bind at line {line_number}.");
            keybindings.push(binding);
          }
          Err(err) => {
            warn!("Line {line_number}: unable to translate Hyprland bind '{value}': {err}");
          }
        }
      }
      DirectiveKind::ExecOnce | DirectiveKind::ExecShutdown => {
        let command_tokens: Vec<String> =
          value.split_whitespace().map(str::to_string).collect();

        if command_tokens.is_empty() {
          continue;
        }

        debug!("Translated Hyprland exec directive at line {line_number}.");

        let command = InvokeCommand::ShellExec {
          hide_window: false,
          command: command_tokens,
        };

        if kind == DirectiveKind::ExecShutdown {
          shutdown_commands.push(command);
        } else {
          startup_commands.push(command);
        }
      }
      DirectiveKind::Submap => {
        info!(
          "Line {line_number}: 'submap' is unsupported; binding will \
be treated as global."
        );
      }
      DirectiveKind::StrippedUnsupported => {
        info!(
          "Line {line_number}: ignoring unsupported Hyprland directive \
'{directive_name}'."
        );
      }
    }
  }

  // Preserve a trailing newline so round-tripping doesn't alter files
  // that legitimately end with one.
  let mut stripped_str = stripped_lines.join("\n");
  if config_str.ends_with('\n') {
    stripped_str.push('\n');
  }

  (
    stripped_str,
    HyprlandDirectives {
      keybindings,
      startup_commands,
      shutdown_commands,
    },
  )
}

/// Parses a config line into a `(directive name, value)` pair if the
/// line matches a Hyprland-style directive.
///
/// A valid directive is a line whose leading non-whitespace characters
/// form a bare identifier (e.g. `bind`, `exec-once`,
/// `windowrulev2:opt`) followed by `= value`. Regular YAML lines (keys
/// ending in `:` or nested values like `- 'some string'`) never match,
/// even when they contain `=` inside a quoted value.
fn parse_directive_line(line: &str) -> Option<(&str, String)> {
  let trimmed_start = line.trim_start();
  let eq_index = trimmed_start.find('=')?;

  let raw_name = trimmed_start[..eq_index].trim_end();

  // The name must be a plain identifier (optionally followed by a
  // sub-property) to distinguish directives from regular YAML content.
  let name = if let Some(stripped) = raw_name.strip_prefix('-') {
    stripped.trim_start()
  } else {
    raw_name
  };

  let is_identifier = !name.is_empty()
    && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
    && name
      .chars()
      .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
    && !name.contains(' ');

  // Skip key-value pairs that are clearly part of regular YAML (e.g.
  // `commands: [...]` or nested maps).
  let ends_in_colon = name.ends_with(':');

  if !is_identifier || ends_in_colon {
    return None;
  }

  let value = strip_inline_comment(trimmed_start[eq_index + 1..].trim());

  if value.is_empty() {
    return None;
  }

  Some((name, value))
}

/// Classifies a directive name into its [`DirectiveKind`].
fn classify_directive(name: &str) -> DirectiveKind {
  let lower_name = name.to_ascii_lowercase();

  match lower_name.as_str() {
    n if n.starts_with("bind") && n != "bindings" => DirectiveKind::Bind,
    "exec-once" | "execr-once" => DirectiveKind::ExecOnce,
    "exec-shutdown" | "execr-shutdown" => DirectiveKind::ExecShutdown,
    "submap" => DirectiveKind::Submap,
    _ => DirectiveKind::StrippedUnsupported,
  }
}

/// Removes a trailing comment (`# ...`) from a directive value while
/// respecting quoted sections.
#[must_use]
fn strip_inline_comment(value: &str) -> String {
  let mut in_single_quote = false;
  let mut in_double_quote = false;

  for (index, char) in value.char_indices() {
    match char {
      '\'' if !in_double_quote => in_single_quote = !in_single_quote,
      '"' if !in_single_quote => in_double_quote = !in_double_quote,
      '#' if !in_single_quote && !in_double_quote => {
        return value[..index].trim_end().to_string();
      }
      _ => {}
    }
  }

  value.to_string()
}

/// Translates a `bind` directive value into a [`KeybindingConfig`].
///
/// Expects the format `[MODS,] key, dispatcher[, args...]`.
fn translate_bind(value: &str) -> Result<KeybindingConfig, String> {
  let segments: Vec<&str> = value.split(',').map(str::trim).collect();
  let mut modifier_keys: Vec<Key> = Vec::new();
  let mut segment_index = 0;

  // Collect leading segments composed entirely of modifier tokens.
  while let Some(segment) = segments.get(segment_index) {
    let tokens: Vec<String> = segment
      .split(|c: char| c.is_whitespace() || c == '_')
      .filter(|token| !token.is_empty())
      .map(str::to_ascii_lowercase)
      .collect();

    if tokens.is_empty() {
      segment_index += 1;
      continue;
    }

    // Stop once a segment contains a non-modifier token (i.e. the key).
    if !tokens
      .iter()
      .all(|token| parse_modifier_token(token).is_some())
    {
      break;
    }

    modifier_keys.extend(
      tokens.iter().filter_map(|token| parse_modifier_token(token)),
    );
    segment_index += 1;
  }

  if modifier_keys.len() > MAX_MODIFIERS_PER_BIND {
    return Err(format!(
      "Too many modifiers in bind '{value}'."
    ));
  }

  let trigger_key_str =
    segments.get(segment_index).ok_or_else(|| format!("Missing key in bind '{value}'."))?;

  let trigger_key = parse_trigger_key(trigger_key_str)?;

  let dispatch_segments = segments.get(segment_index + 1..).ok_or_else(|| {
    format!("Missing dispatcher in bind '{value}'.")
  })?;

  let dispatch_str = dispatch_segments.join(",");
  let dispatch_words_str = dispatch_str.replace(',', " ");
  let mut dispatch_words = dispatch_words_str.split_whitespace();

  // Dispatcher name (e.g. `killactive`, `exec`).
  let dispatcher_name = dispatch_words
    .next()
    .ok_or_else(|| format!("Missing dispatcher in bind '{value}'."))?;

  let dispatcher_args: Vec<String> =
    dispatch_words.map(str::to_string).collect();

  let command =
    translate_dispatcher(dispatcher_name, &dispatcher_args)?;

  let mut keys = modifier_keys;
  keys.push(trigger_key);

  let binding =
    Keybinding::new(keys).map_err(|_| format!("Invalid keybinding '{value}'."))?;

  Ok(KeybindingConfig {
    bindings: vec![binding],
    commands: vec![command],
  })
}

/// Maximum number of modifiers allowed per bind.
const MAX_MODIFIERS_PER_BIND: usize = 5;

/// Maps a Hyprland modifier token to a corresponding [`Key`].
///
/// Tokens are lowercased before being passed here.
#[must_use]
fn parse_modifier_token(token: &str) -> Option<Key> {
  match token {
    "super" | "win" | "mod2" => Some(Key::Win),
    "superl" | "lwin" => Some(Key::LWin),
    "superr" | "rwin" => Some(Key::RWin),
    "shift" | "mod5" => Some(Key::Shift),
    "shiftl" => Some(Key::LShift),
    "shiftr" => Some(Key::RShift),
    "ctrl" | "control" | "mod4" => Some(Key::Ctrl),
    "ctrll" | "lctrl" => Some(Key::LCtrl),
    "ctrlr" | "rctrl" => Some(Key::RCtrl),
    "alt" | "mod1" => Some(Key::Alt),
    "altl" | "lalt" => Some(Key::LAlt),
    "altr" | "ralt" => Some(Key::RAlt),
    _ => None,
  }
}

/// Parses a trigger key token (e.g. `Q`, `Return`, `minus`, `KP_Add`).
///
/// Hyprland names keys after their X11 keysym, so punctuation, numpad and
/// media keys are spelled out (e.g. `minus`, `bracketleft`,
/// `XF86AudioRaiseVolume`) rather than written literally. Those names are
/// resolved via [`parse_keysym`]; anything else falls back to `GlazeWM`'s
/// own key names and then to a single literal character (e.g. `-`).
///
/// Mouse buttons are unsupported and result in an error.
fn parse_trigger_key(token: &str) -> Result<Key, String> {
  if token.to_ascii_lowercase().replace('_', "").starts_with("mouse") {
    return Err(format!(
      "Mouse bindings (e.g. '{token}') are unsupported."
    ));
  }

  if let Some(key) = parse_keysym(token) {
    return Ok(key);
  }

  if let Ok(key) = token.parse::<Key>() {
    return Ok(key);
  }

  // `Key::try_from_literal` only inspects the first character of the
  // token, so restrict it to single-character keys. Otherwise an
  // unrecognized keysym such as `minus` would silently bind to `m`.
  let is_literal = token.chars().count() == 1;

  if is_literal {
    if let Ok(key) = Key::try_from_literal(token) {
      return Ok(key);
    }
  }

  Err(format!("Unknown key '{token}'."))
}

/// Maps an X11/Hyprland keysym name to its corresponding [`Key`].
///
/// Only keysyms whose name differs from `GlazeWM`'s own key names are
/// listed; the rest (e.g. `space`, `tab`, `escape`, `f1`) already parse
/// via `Key`'s `FromStr`.
///
/// # Platform-specific
///
/// - **Windows/macOS**: Punctuation keysyms map to the OEM key at that
///   position on a US layout. On other layouts the physical key may
///   differ, matching how `GlazeWM` treats OEM keys generally.
#[must_use]
fn parse_keysym(token: &str) -> Option<Key> {
  // Keysyms are matched case-insensitively, and `KP_Add`/`kp_add` are
  // treated the same.
  let name = token.to_ascii_lowercase();

  let key = match name.as_str() {
    // Punctuation. Shifted spellings (e.g. `plus`, `underscore`) map to
    // the same physical key as their unshifted counterpart, since a
    // binding's modifiers are declared separately.
    "minus" | "underscore" => Key::OemMinus,
    "equal" | "plus" => Key::OemPlus,
    "comma" | "less" => Key::OemComma,
    "period" | "greater" => Key::OemPeriod,
    "slash" | "question" => Key::OemQuestion,
    "semicolon" | "colon" => Key::OemSemicolon,
    "apostrophe" | "quotedbl" => Key::OemQuotes,
    "grave" | "asciitilde" => Key::OemTilde,
    "bracketleft" | "braceleft" => Key::OemOpenBrackets,
    "bracketright" | "braceright" => Key::OemCloseBrackets,
    "backslash" | "bar" => Key::OemPipe,

    // Keys whose keysym name differs from the `GlazeWM` name.
    "return" => Key::Enter,
    "prior" | "page_up" => Key::PageUp,
    "next" | "page_down" => Key::PageDown,
    "print" => Key::PrintScreen,
    "caps_lock" => Key::CapsLock,
    "num_lock" => Key::NumLock,
    "scroll_lock" => Key::ScrollLock,

    // Numpad.
    "kp_add" => Key::NumpadAdd,
    "kp_subtract" => Key::NumpadSubtract,
    "kp_multiply" => Key::NumpadMultiply,
    "kp_divide" => Key::NumpadDivide,
    "kp_decimal" | "kp_separator" => Key::NumpadDecimal,
    "kp_0" | "kp_insert" => Key::Numpad0,
    "kp_1" | "kp_end" => Key::Numpad1,
    "kp_2" | "kp_down" => Key::Numpad2,
    "kp_3" | "kp_next" => Key::Numpad3,
    "kp_4" | "kp_left" => Key::Numpad4,
    "kp_5" | "kp_begin" => Key::Numpad5,
    "kp_6" | "kp_right" => Key::Numpad6,
    "kp_7" | "kp_home" => Key::Numpad7,
    "kp_8" | "kp_up" => Key::Numpad8,
    "kp_9" | "kp_prior" => Key::Numpad9,

    // Media keys.
    "xf86audioraisevolume" => Key::VolumeUp,
    "xf86audiolowervolume" => Key::VolumeDown,
    "xf86audiomute" => Key::VolumeMute,
    "xf86audionext" => Key::MediaNextTrack,
    "xf86audioprev" => Key::MediaPrevTrack,
    "xf86audiostop" => Key::MediaStop,
    "xf86audioplay" | "xf86audiopause" => Key::MediaPlayPause,

    _ => return None,
  };

  Some(key)
}

/// Translates a Hyprland dispatcher name and its arguments into a
/// corresponding [`InvokeCommand`].
fn translate_dispatcher(
  name: &str,
  args: &[String],
) -> Result<InvokeCommand, String> {
  let name_lower = name.to_ascii_lowercase();

  match name_lower.as_str() {
    "killactive" | "closewindow" | "closewindowconfirm" => {
      Ok(InvokeCommand::Close)
    }
    "exit" | "quit" => Ok(InvokeCommand::WmExit),
    "reload" | "forcerendererreload" => Ok(InvokeCommand::WmReloadConfig),
    "togglesplit" => Ok(InvokeCommand::ToggleTilingDirection),
    "togglefloating" => Ok(InvokeCommand::ToggleFloating {
      shown_on_top: None,
      centered: None,
    }),
    "fullscreen" => match args.first().map(String::as_str) {
      Some("1" | "maximized") => Ok(InvokeCommand::ToggleFullscreen {
        shown_on_top: None,
        maximized: Some(true),
      }),
      Some("2" | "ontop") => Ok(InvokeCommand::ToggleFullscreen {
        shown_on_top: Some(true),
        maximized: None,
      }),
      _ => Ok(InvokeCommand::ToggleFullscreen {
        shown_on_top: None,
        maximized: None,
      }),
    },
    "movefocus" => Ok(InvokeCommand::Focus(
      InvokeFocusCommand {
        direction: Some(parse_direction(args.first())?),
        ..Default::default()
      },
    )),
    "movewindow" => Ok(InvokeCommand::Move(InvokeMoveCommand {
      direction: Some(parse_direction(args.first())?),
      ..Default::default()
    })),
    "swapwindow" => Ok(InvokeCommand::Swap {
      direction: parse_direction(args.first())?,
    }),
    "movetoworkspace" | "movetoworkspacesilent" => {
      translate_move_to_workspace(args)
    }
    "workspace" => translate_workspace_focus(args),
    "focusmonitor" => {
      let index = args
        .first()
        .and_then(|arg| arg.parse::<usize>().ok())
        .ok_or_else(|| format!("Invalid monitor index for '{name}'."))?;

      Ok(InvokeCommand::Focus(InvokeFocusCommand {
        monitor: Some(index),
        ..Default::default()
      }))
    }
    "cyclenext" | "cycleprev" => Ok(InvokeCommand::WmCycleFocus {
      omit_floating: false,
      omit_fullscreen: false,
      omit_minimized: true,
      omit_tiling: false,
    }),
    "resizeactive" => {
      let width = parse_resize_delta(args.first())?;
      let height = parse_resize_delta(args.get(1))?;

      if width == 0 && height == 0 {
        return Err(
          "Resize deltas for 'resizeactive' cannot both be zero.".into(),
        );
      }

      // Zero deltas are omitted so that the resize only touches the
      // intended axis, rather than also re-applying the current size on
      // the other one.
      Ok(InvokeCommand::Resize(InvokeResizeCommand {
        width: (width != 0).then(|| LengthValue::from_px(width)),
        height: (height != 0).then(|| LengthValue::from_px(height)),
      }))
    }
    "sendshortcut" | "sendkeys" => translate_send_shortcut(args),
    "exec" | "execr" | "shell" => Ok(InvokeCommand::ShellExec {
      hide_window: false,
      command: args.to_vec(),
    }),
    _ => Err(format!("Unsupported Hyprland dispatcher '{name}'.")),
  }
}

/// Translates the `movetoworkspace` / `movetoworkspacesilent`
/// dispatchers.
///
/// Flags (e.g. `silent`, `new`) are filtered out; only the workspace
/// name matters.
fn translate_move_to_workspace(
  args: &[String],
) -> Result<InvokeCommand, String> {
  let target = args
    .iter()
    .find(|arg| {
      !matches!(arg.as_str(), "silent" | "opt-in" | "opt-out" | "new")
    })
    .ok_or_else(|| "Missing workspace argument.".to_string())?;

  Ok(InvokeCommand::Move(InvokeMoveCommand {
    workspace: Some(target.clone()),
    ..Default::default()
  }))
}

/// Translates the `sendshortcut` dispatcher.
///
/// Hyprland's form is `sendshortcut, MODS, key, window`. The trailing
/// window argument is unsupported: the shortcut always goes to the
/// focused window, so anything after the key is rejected rather than
/// silently sending to the wrong target.
///
/// Note that the comma structure of the original directive is already
/// flattened by the time this is called, so `CTRL SHIFT, S` and
/// `CTRL, SHIFT, S` both arrive as three tokens.
fn translate_send_shortcut(
  args: &[String],
) -> Result<InvokeCommand, String> {
  let mut keys: Vec<Key> = Vec::new();
  let mut remaining = args.iter();

  // Leading modifier tokens are held for the trigger key's duration.
  let trigger_key = loop {
    let token = remaining
      .next()
      .ok_or_else(|| "Missing key for 'sendshortcut'.".to_string())?;

    match parse_modifier_token(&token.to_ascii_lowercase()) {
      Some(modifier) => keys.push(modifier),
      None => break parse_trigger_key(token)?,
    }
  };

  // Hyprland allows targeting another window; `GlazeWM` always sends to
  // the focused one, so reject rather than mislead.
  if let Some(window_arg) = remaining.next() {
    if !matches!(window_arg.as_str(), "activewindow" | "active") {
      return Err(format!(
        "Targeting a specific window ('{window_arg}') is unsupported \
for 'sendshortcut'; it always applies to the focused window."
      ));
    }
  }

  keys.push(trigger_key);

  Ok(InvokeCommand::SendShortcut { keys })
}

/// Translates the `workspace` dispatcher into a focus command.
///
/// Supports explicit workspace names as well as the relative targets
/// `next` / `previous`.
fn translate_workspace_focus(args: &[String]) -> Result<InvokeCommand, String> {
  let target = args
    .first()
    .ok_or_else(|| "Missing workspace argument.".to_string())?;

  match target.to_ascii_lowercase().as_str() {
    "next" => Ok(InvokeCommand::Focus(InvokeFocusCommand {
      next_workspace: true,
      ..Default::default()
    })),
    "previous" | "prev" => Ok(InvokeCommand::Focus(InvokeFocusCommand {
      prev_workspace: true,
      ..Default::default()
    })),
    _ => Ok(InvokeCommand::Focus(InvokeFocusCommand {
      workspace: Some(target.clone()),
      ..Default::default()
    })),
  }
}

/// Parses a directional argument (e.g. `l`, `left`) into a
/// [`Direction`].
fn parse_direction(arg: Option<&String>) -> Result<Direction, String> {
  let Some(direction) = arg else {
    return Err("Missing direction argument.".into());
  };

  match direction.to_ascii_lowercase().as_str() {
    "l" | "left" => Ok(Direction::Left),
    "r" | "right" => Ok(Direction::Right),
    "u" | "up" => Ok(Direction::Up),
    "d" | "down" => Ok(Direction::Down),
    _ => Err(format!("Unknown direction '{direction}'.")),
  }
}

/// Parses a resize delta argument (in pixels).
fn parse_resize_delta(arg: Option<&String>) -> Result<i32, String> {
  let Some(delta) = arg else {
    return Err("Missing resize delta.".into());
  };

  delta.parse::<i32>().map_err(|_| {
    format!("Invalid resize delta '{delta}'. Expected pixel values (e.g. '-30').")
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_bind_translation_killactive() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, Q, killactive\n");

    assert_eq!(directives.keybindings.len(), 1);

    let binding = &directives.keybindings[0];
    let keys = binding.bindings[0].keys();

    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], Key::Win);
    assert_eq!(keys[1], Key::Q);
    assert_eq!(binding.commands[0], InvokeCommand::Close);
  }

  #[test]
  fn test_bind_with_space_separated_modifiers() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER SHIFT, V, togglefloating\n");

    assert_eq!(directives.keybindings.len(), 1);

    let keys = directives.keybindings[0].bindings[0].keys();

    assert_eq!(keys, &[Key::Win, Key::Shift, Key::V]);
  }

  #[test]
  fn test_binde_with_no_modifiers() {
    let (_, directives) =
      extract_hyprland_directives("binde = , right, movefocus r\n");

    let keys = directives.keybindings[0].bindings[0].keys();

    assert_eq!(keys, &[Key::Right]);
  }

  #[test]
  fn test_return_alias() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, Return, cyclenext\n");

    let keys = directives.keybindings[0].bindings[0].keys();

    assert_eq!(keys, &[Key::Win, Key::Enter]);
  }

  #[test]
  fn test_exec_once_startup_commands() {
    let (_, directives) =
      extract_hyprland_directives("exec-once = wezterm --cwd ~/projects\n");

    assert_eq!(directives.startup_commands.len(), 1);

    let InvokeCommand::ShellExec { hide_window, command } =
      &directives.startup_commands[0]
    else {
      panic!("Expected ShellExec.");
    };

    assert!(!hide_window);
    assert_eq!(command, &["wezterm", "--cwd", "~/projects"]);
  }

  #[test]
  fn test_exec_shutdown() {
    let (_, directives) =
      extract_hyprland_directives("exec-shutdown = notify-send bye\n");

    assert_eq!(directives.shutdown_commands.len(), 1);
  }

  #[test]
  fn test_movetoworkspace_translation() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, 3, movetoworkspace, 3\n");

    let InvokeCommand::Move(InvokeMoveCommand { workspace, .. }) =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected Move.");
    };

    assert_eq!(workspace.as_deref(), Some("3"));
  }

  #[test]
  fn test_workspace_focus_translation() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, 3, workspace, 3\n");

    let InvokeCommand::Focus(InvokeFocusCommand { workspace, .. }) =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected Focus.");
    };

    assert_eq!(workspace.as_deref(), Some("3"));
  }

  #[test]
  fn test_movefocus_directions() {
    for (direction_arg, expected) in [
      ("l", Direction::Left),
      ("r", Direction::Right),
      ("u", Direction::Up),
      ("d", Direction::Down),
    ] {
      let config_str =
        format!("bind = SUPER, K, movefocus, {direction_arg}\n");

      let (_, directives) = extract_hyprland_directives(&config_str);

      let InvokeCommand::Focus(InvokeFocusCommand { direction, .. }) =
        &directives.keybindings[0].commands[0]
      else {
        panic!("Expected Focus.");
      };

      assert_eq!(direction, &Some(expected));
    }
  }

  #[test]
  fn test_togglesplit_translation() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, J, togglesplit\n");

    assert_eq!(
      directives.keybindings[0].commands[0],
      InvokeCommand::ToggleTilingDirection
    );
  }

  #[test]
  fn test_resizeactive_translation() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER_ALT, L, resizeactive, 30 -20\n");

    let InvokeCommand::Resize(InvokeResizeCommand { width, height }) =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected Resize.");
    };

    assert_eq!(width.as_ref().map(|l| l.amount), Some(30.0));
    assert_eq!(height.as_ref().map(|l| l.amount), Some(-20.0));
  }

  #[test]
  fn test_fullscreen_maximized_variant() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, M, fullscreen, 1\n");

    let InvokeCommand::ToggleFullscreen { maximized, .. } =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected ToggleFullscreen.");
    };

    assert_eq!(maximized, &Some(true));
  }

  #[test]
  fn test_cyclenext_translation() {
    let (_, directives) =
      extract_hyprland_directives("bind = ALT, Tab, cyclenext\n");

    let InvokeCommand::WmCycleFocus { omit_minimized, .. } =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected WmCycleFocus.");
    };

    assert!(omit_minimized);
  }

  #[test]
  fn test_inline_comments_stripped() {
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER, Q, killactive # close focused window\n",
    );

    assert_eq!(directives.keybindings[0].commands[0], InvokeCommand::Close);
  }

  #[test]
  fn test_quoted_hash_preserved() {
    let (_, directives) = extract_hyprland_directives(
      "exec-once = pwsh -Command echo '#hashtag'\n",
    );

    let InvokeCommand::ShellExec { command, .. } =
      &directives.startup_commands[0]
    else {
      panic!("Expected ShellExec.");
    };

    assert!(
      command.iter().any(|token| token.contains('#')),
      "Quoted hash was incorrectly stripped."
    );
  }

  #[test]
  fn test_mouse_bindings_skipped() {
    let (_, directives) =
      extract_hyprland_directives("bindm = SUPER, mouse:272, movewindow\n");

    assert!(directives.keybindings.is_empty());
  }

  #[test]
  fn test_unsupported_directives_stripped() {
    let config_str = concat!(
      "windowrulev2 = float, class:^(kitty)$\n",
      "env = XCURSOR_SIZE,24\n"
    );

    let (stripped, directives) = extract_hyprland_directives(config_str);

    assert!(directives.keybindings.is_empty());
    assert!(!stripped.contains("windowrulev2"));
    assert!(!stripped.contains("env ="));
  }

  #[test]
  fn test_submap_stripped_with_binds_kept_global() {
    let config_str = concat!(
      "submap = resize\n",
      "binde = , right, resizeactive, 10 0\n",
      "submap = reset\n",
      "bind = SUPER, Q, killactive\n"
    );

    let (stripped, directives) = extract_hyprland_directives(config_str);

    assert_eq!(directives.keybindings.len(), 2);
    assert!(!stripped.contains("submap"));
  }

  #[test]
  fn test_regular_yaml_lines_untouched() {
    let config_str = concat!(
      "general:\n",
      "  focus_follows_cursor: true\n",
      "\n",
      "keybindings:\n",
      "  - commands: ['wm-disable-binding-mode --name resize']\n",
      "    bindings: ['escape']\n"
    );

    let (stripped, _) = extract_hyprland_directives(config_str);

    assert_eq!(stripped, config_str);
  }

  #[test]
  fn test_mixed_config_produces_valid_yaml() {
    let config_str = concat!(
      "# Hyprland-flavored config.\n",
      "gaps:\n",
      "  inner_gap: '8px'\n",
      "\n",
      "bind = SUPER, Q, killactive\n",
      "bind = SUPER, 1, workspace, 1\n",
      "exec-once = wezterm\n",
      "\n",
      "window_effects:\n",
      "  focused_window:\n",
      "    border:\n",
      "      enabled: true\n"
    );

    let (stripped, directives) = extract_hyprland_directives(config_str);

    assert_eq!(directives.keybindings.len(), 2);
    assert_eq!(directives.startup_commands.len(), 1);

    assert!(stripped.contains("gaps:"));
    assert!(stripped.contains("inner_gap: '8px'"));
    assert!(stripped.contains("window_effects:"));
    assert!(!stripped.contains("bind = "));
  }

  #[test]
  fn test_unknown_key_is_skipped_gracefully() {
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER, XF86XK_TurboFake, killactive\n",
    );

    // Untranslatable binds are dropped rather than breaking the config.
    assert!(directives.keybindings.is_empty());
  }

  #[test]
  fn test_swapwindow_translation() {
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER SHIFT, left, swapwindow, l\n",
    );

    assert_eq!(
      directives.keybindings[0].commands[0],
      InvokeCommand::Swap {
        direction: Direction::Left
      }
    );

    let keys = directives.keybindings[0].bindings[0].keys();

    assert_eq!(keys, &[Key::Win, Key::Shift, Key::Left]);
  }

  #[test]
  fn test_movewindow_is_not_a_swap() {
    // `movewindow` re-parents the window (and can leave the workspace),
    // whereas `swapwindow` only trades places with a neighbor.
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER SHIFT, H, movewindow, l\n",
    );

    assert!(matches!(
      directives.keybindings[0].commands[0],
      InvokeCommand::Move(_)
    ));
  }

  #[test]
  fn test_sendshortcut_translation() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, C, sendshortcut, CTRL, C,\n");

    let InvokeCommand::SendShortcut { keys } =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected SendShortcut.");
    };

    assert_eq!(keys, &[Key::Ctrl, Key::C]);
  }

  #[test]
  fn test_sendshortcut_with_multiple_modifiers() {
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER, S, sendshortcut, CTRL SHIFT, S,\n",
    );

    let InvokeCommand::SendShortcut { keys } =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected SendShortcut.");
    };

    assert_eq!(keys, &[Key::Ctrl, Key::Shift, Key::S]);
  }

  #[test]
  fn test_sendshortcut_with_keysym_key() {
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER, slash, sendshortcut, CTRL, slash,\n",
    );

    let InvokeCommand::SendShortcut { keys } =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected SendShortcut.");
    };

    assert_eq!(keys, &[Key::Ctrl, Key::OemQuestion]);
  }

  #[test]
  fn test_sendshortcut_rejects_window_target() {
    // Targeting another window is unsupported and must not be silently
    // downgraded to sending at the focused window.
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER, C, sendshortcut, CTRL, C, class:^(kitty)$\n",
    );

    assert!(directives.keybindings.is_empty());
  }

  #[test]
  fn test_sendshortcut_requires_a_key() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, C, sendshortcut, CTRL,\n");

    assert!(directives.keybindings.is_empty());
  }

  #[test]
  fn test_punctuation_keysyms() {
    for (keysym, expected) in [
      ("minus", Key::OemMinus),
      ("equal", Key::OemPlus),
      ("plus", Key::OemPlus),
      ("comma", Key::OemComma),
      ("period", Key::OemPeriod),
      ("slash", Key::OemQuestion),
      ("bracketleft", Key::OemOpenBrackets),
      ("bracketright", Key::OemCloseBrackets),
      ("grave", Key::OemTilde),
      ("backslash", Key::OemPipe),
      ("Print", Key::PrintScreen),
      ("Prior", Key::PageUp),
      ("KP_Add", Key::NumpadAdd),
      ("XF86AudioRaiseVolume", Key::VolumeUp),
    ] {
      let config_str = format!("bind = SUPER, {keysym}, killactive\n");

      let (_, directives) = extract_hyprland_directives(&config_str);

      let keys = directives
        .keybindings
        .first()
        .unwrap_or_else(|| panic!("Keysym '{keysym}' was not translated."))
        .bindings[0]
        .keys();

      assert_eq!(keys, &[Key::Win, expected], "Wrong key for '{keysym}'.");
    }
  }

  #[test]
  fn test_multi_char_keysym_does_not_fall_back_to_first_letter() {
    // `Key::try_from_literal` only inspects the first character, so an
    // unknown keysym must be rejected rather than silently binding to it.
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, dead_acute, killactive\n");

    assert!(directives.keybindings.is_empty());
  }

  #[test]
  fn test_single_literal_key_still_parses() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, -, killactive\n");

    let keys = directives.keybindings[0].bindings[0].keys();

    assert_eq!(keys, &[Key::Win, Key::OemMinus]);
  }

  #[test]
  fn test_resizeactive_omits_zero_delta() {
    let (_, directives) =
      extract_hyprland_directives("binde = SUPER, minus, resizeactive, -100 0\n");

    let InvokeCommand::Resize(InvokeResizeCommand { width, height }) =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected Resize.");
    };

    assert_eq!(width.as_ref().map(|l| l.amount), Some(-100.0));
    assert!(height.is_none(), "Zero height delta should be omitted.");
  }

  #[test]
  fn test_resizeactive_rejects_all_zero_deltas() {
    let (_, directives) =
      extract_hyprland_directives("binde = SUPER, minus, resizeactive, 0 0\n");

    assert!(directives.keybindings.is_empty());
  }

  #[test]
  fn test_focusmonitor_translation() {
    let (_, directives) =
      extract_hyprland_directives("bind = SUPER, F2, focusmonitor, 1\n");

    let InvokeCommand::Focus(InvokeFocusCommand { monitor, .. }) =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected Focus.");
    };

    assert_eq!(monitor, &Some(1));
  }

  #[test]
  fn test_movetoworkspace_silent_flag_filtered() {
    let (_, directives) = extract_hyprland_directives(
      "bind = SUPER_SHIFT, 3, movetoworkspace, 3, silent\n",
    );

    let InvokeCommand::Move(InvokeMoveCommand { workspace, .. }) =
      &directives.keybindings[0].commands[0]
    else {
      panic!("Expected Move.");
    };

    assert_eq!(workspace.as_deref(), Some("3"));
  }
}



#[cfg(test)]
mod example_config_tests {
  use super::*;

  use crate::ParsedConfig;

  /// The hyprland-flavored example config shipped in `docs/` must parse
  /// cleanly through the directive extraction + YAML pipeline.
  #[test]
  fn test_example_config_loads_end_to_end() {
    let config_str =
      include_str!("../../../docs/hyprland-flavored-config.yaml");

    let (stripped, directives) = extract_hyprland_directives(config_str);

    // Binds defined in the example (plus resize-mode helper).
    assert!(
      directives.keybindings.len() >= 20,
      "Expected at least 20 translated binds.",
    );

    assert_eq!(directives.startup_commands.len(), 2);

    let parsed: ParsedConfig = serde_yaml::from_str(&stripped)
      .expect("Stripped example config should be valid YAML.");

    assert_eq!(parsed.keybindings.len(), 0);
    assert_eq!(parsed.binding_modes.len(), 1);
    assert_eq!(
      parsed.binding_modes[0].keybindings.len(),
      5,
      "Native binding-mode keybindings must survive directive stripping.",
    );
    assert_eq!(parsed.gaps.inner_gap.amount, 8.0);
  }
}

