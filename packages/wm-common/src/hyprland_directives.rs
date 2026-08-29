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

/// Parses a trigger key token (e.g. `Q`, `Return`, `KP_Add`).
///
/// Mouse buttons are unsupported and result in an error.
fn parse_trigger_key(token: &str) -> Result<Key, String> {
  if token.to_ascii_lowercase().replace('_', "").starts_with("mouse") {
    return Err(format!(
      "Mouse bindings (e.g. '{token}') are unsupported."
    ));
  }

  if token.to_ascii_lowercase().as_str() == "return" { return Ok(Key::Enter) }

  token
    .parse::<Key>()
    .or_else(|_| Key::try_from_literal(token))
    .map_err(|err| format!("Unknown key '{token}': {err}."))
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
    "movewindow" | "swapwindow" => Ok(InvokeCommand::Move(
      InvokeMoveCommand {
        direction: Some(parse_direction(args.first())?),
        ..Default::default()
      },
    )),
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

      Ok(InvokeCommand::Resize(InvokeResizeCommand {
        width: Some(LengthValue::from_px(width)),
        height: Some(LengthValue::from_px(height)),
      }))
    }
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

  use crate::ParsedConfig;

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

