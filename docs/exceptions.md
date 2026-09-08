# Exceptions

Where this fork's Hyprland-flavored behavior does not, or cannot, match
Hyprland on Windows. Read this before filing a bug — most entries here
are OS constraints rather than defects.

## Keyboard interception

### Reserved combos that cannot be intercepted

Windows handles these below the low-level keyboard hook, so no window
manager running as a normal process can rebind or suppress them:

| Combo | Handled by |
| --- | --- |
| `Win+L` | Workstation lock |
| `Ctrl+Alt+Del` | Secure attention sequence |
| `Ctrl+Shift+Esc` | Task Manager (usually interceptable, not guaranteed) |

If your Hyprland keymap uses `SUPER+L` for anything, it needs a different
key here. Our example config uses `SUPER+L` for `movefocus, r`, which
means it is shadowed by the lock shortcut — remap it if you rely on it.

### Elevated windows reject synthesized input

`sendshortcut` uses `SendInput`, which is subject to User Interface
Privilege Isolation. A process running at normal integrity cannot send
input to a window running elevated. So with GlazeWM launched normally:

- `Win+C` in an admin PowerShell, Registry Editor, Task Manager, or any
  app started with "Run as administrator" does nothing at all — silently.
- Everything else in the same session works fine.

Running GlazeWM elevated fixes it, at the cost of running your window
manager as administrator. That is a real trade-off, not a recommendation.

### Shell hotkeys we take over

These are claimed by the Windows shell but *are* interceptable, so
binding them in your config reclaims them:

| Combo | Normally does | Reclaimed by binding it |
| --- | --- | --- |
| `Win+C` | Copilot | yes |
| `Win+V` | Clipboard history | yes |
| `Win+-` / `Win+=` | Magnifier zoom | yes |
| `Win+X` | Quick Link menu | yes |
| `Win+A` | Quick Settings | yes |
| `Win+S` | Search | yes |

The catch is that reclaiming only holds while GlazeWM runs. If it exits,
the shell behavior comes back. That is usually what you want, but it does
mean a crash silently restores Copilot on `Win+C`.

Optional belt-and-braces, neither required nor scoped to GlazeWM:

```powershell
# Disable clipboard history (per-user, no admin, reversible with 1).
Set-ItemProperty 'HKCU:\Software\Microsoft\Clipboard' EnableClipboardHistory 0

# Ask Windows to turn off Copilot. Documented, but deprecated since
# Copilot became a regular app -- may be a no-op on 24H2 and later.
New-Item -Path 'HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot' -Force
Set-ItemProperty 'HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot' TurnOffWindowsCopilot 1
```

There is **no** user-level registry toggle for the Magnifier hotkey. The
only reliable non-hook method is an Image File Execution Options entry on
`Magnify.exe`, which requires administrator.

### The held Win key

When `Win+C` fires, the physical `Win` key is still down. `send_keys`
therefore presses the target chord's modifiers *first*, then releases the
held `Win`, so the focused app sees `Ctrl+C` rather than `Win+Ctrl+C`, and
the shell treats the `Win` press as consumed and does not open the Start
menu.

This ordering is a heuristic against undocumented shell behavior. If the
Start menu flickers on certain shortcuts, that is why.

### Numpad `+` and `-` are separate keys

`minus` and `equal` map to the main-row `VK_OEM_MINUS` / `VK_OEM_PLUS`.
The numpad keys are distinct — bind `KP_Subtract` and `KP_Add` if you
want both.

### Punctuation follows the US layout

X11 keysym names (`minus`, `bracketleft`, `slash`, ...) map to the OEM
virtual key at that position on a US keyboard. On other layouts the
physical key may differ. This matches how GlazeWM treats OEM keys
generally.

## Unsupported `hyprland.conf` directives

Translated on a best-effort basis. Anything not listed below is stripped
with a log line rather than failing the config parse.

| Directive | Status |
| --- | --- |
| `cycleprev` | Cycles *forward*; `wm-cycle-focus` has no direction |
| `focusmonitor` | Numeric index only — `+1`, `l`, `r` are rejected |
| `workspace, e+1` / `r+1` | Relative targets unhandled; read as a literal name |
| `exec` with quotes | Args split on whitespace, so quoting is lost |
| `movetoworkspace` | Does not follow focus; behaves as `movetoworkspacesilent` |
| `sendshortcut` window arg | Rejected; always applies to the focused window |
| `bindm` (mouse binds) | Unsupported |
| `submap` | Stripped; its binds stay global |
| `windowrulev2`, `env`, ... | Stripped with a log line |

## Platform

`sendshortcut` is Windows-only. On macOS it returns an error rather than
silently doing nothing — the fork's dwindle layout and directive parsing
work there, but synthesized shortcuts do not.

## Verification status

The layout and config-translation logic is covered by unit tests. The
input-synthesis path (`send_keys`) and hotkey reclamation are **not** —
they need a live desktop and have only been reasoned through, not
measured. Treat the shell-hotkey table above as expected behavior rather
than confirmed behavior until you have tried it.
