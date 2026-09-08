# Windows setup

System tweaks you may want beyond just running this fork. None are
required to tile windows — they exist because a few keyboard shortcuts
are claimed by Windows itself, and reclaiming those needs a little help.

For the things that *cannot* be fixed at all, see
[`docs/exceptions.md`](docs/exceptions.md).

## 1. Run it

```powershell
cargo build --release -p wm
.\target\release\glazewm.exe
```

Config lives at `%USERPROFILE%\.glzr\glazewm\config.yaml`. Copy
[`docs/hyprland-flavored-config.yaml`](docs/hyprland-flavored-config.yaml)
there to start from the Hyprland-flavored defaults. You can paste
`hyprland.conf` directives (`bind`, `binde`, `exec-once`) straight into
that file alongside the regular YAML.

Errors are logged to `%USERPROFILE%\.glzr\glazewm\errors.log`. Check
there first when something misbehaves.

## 2. Start on login

Task Scheduler is the reliable route, because a Startup-folder shortcut
starts the WM before the shell is fully up.

```powershell
$exe = "$PWD\target\release\glazewm.exe"
$action  = New-ScheduledTaskAction -Execute $exe
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries `
  -DontStopIfGoingOnBatteries -ExecutionTimeLimit 0
Register-ScheduledTask -TaskName 'GlazeWM' -Action $action `
  -Trigger $trigger -Settings $settings
```

Remove with `Unregister-ScheduledTask -TaskName 'GlazeWM'`.

## 3. Free up shell shortcuts

The `sendshortcut` dispatcher lets you remap `SUPER` combos onto their
`CTRL` equivalents, so `Win+C` copies instead of opening Copilot:

```
bind = SUPER, C, sendshortcut, CTRL, C,
bind = SUPER, V, sendshortcut, CTRL, V,
```

GlazeWM's keyboard hook suppresses the original combo while it runs, so
this needs no system-wide remapping and reverts the moment the WM exits.
Some shell shortcuts fight back harder than others.

### Clipboard history (`Win+V`)

Per-user, no admin:

```powershell
Set-ItemProperty 'HKCU:\Software\Microsoft\Clipboard' EnableClipboardHistory 0
```

Revert with `1`.

### Copilot (`Win+C`)

**Check which Copilot you have** — the fix differs and the widely-quoted
registry key only covers the old one:

```powershell
Get-Process | Where-Object { $_.ProcessName -match 'Copilot' } |
  Select-Object ProcessName, Path
```

*Windows Copilot* (the old sidebar). Policy key works, per-user:

```powershell
New-Item -Path 'HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot' -Force
Set-ItemProperty 'HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot' `
  TurnOffWindowsCopilot 1
```

*Microsoft 365 Copilot* (`M365Copilot.exe`, shipped inside
`Microsoft.MicrosoftOfficeHub`). The policy above does **not** apply to
it. Disable the app's own shortcut in its settings, stop it launching at
login, or remove it:

```powershell
Get-AppxPackage Microsoft.MicrosoftOfficeHub | Remove-AppxPackage
```

### Magnifier (`Win+-` / `Win+=`)

There is no user-level registry toggle for the Magnifier hotkey — the
`ScreenMagnifier` key has no shortcut value and `Control Panel\Accessibility`
has no Magnifier entry. Binding the keys in your config is the practical
fix. The only non-hook method is an Image File Execution Options entry on
`Magnify.exe`, which needs administrator and disables Magnifier entirely.

## 4. Elevated windows

`sendshortcut` uses `SendInput`, which User Interface Privilege Isolation
blocks from a normal-integrity process to an elevated window. Running
GlazeWM normally means `Win+C` inside an admin terminal, Registry Editor,
or Task Manager does nothing — silently, with no error.

Running GlazeWM as administrator fixes it. That means your window manager
runs elevated for the whole session, which is a real trade-off; decide
deliberately rather than by default.

## 5. Troubleshooting a shortcut

When a `sendshortcut` bind misbehaves, the useful question is *which half*
is broken — the hook that suppresses the original combo, or the injection
that sends the replacement. Test the injection on its own:

```powershell
# Select some text, then from another terminal:
.\target\release\glazewm.exe command send-shortcut ctrl c
```

- **Works** — injection is fine. Windows is intercepting your `SUPER`
  combo before GlazeWM's hook sees it. Free the shortcut using section 3,
  or bind a different key.
- **Does nothing** — injection is being blocked. Usually the target window
  is elevated (section 4).

Also worth checking:

- `errors.log` is silent on a failed send. A `SendInput` rejection is
  reported, so no entry means the send reported success.
- Confirm the config actually reloaded (`Win+Shift+R`) — GlazeWM reads it
  at startup, so binds added afterward need a reload.

## 6. Known-unresolved

`Win+C` and `Win+V` are still being worked out on at least one machine
where the rest of the shortcut set works. If you hit this, the diagnostic
in section 5 tells us which half is failing — that information is useful,
please include it in an issue.

## 7. Reverting everything

```powershell
Set-ItemProperty 'HKCU:\Software\Microsoft\Clipboard' EnableClipboardHistory 1
Remove-Item 'HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot' -Recurse
Unregister-ScheduledTask -TaskName 'GlazeWM' -Confirm:$false
```

Keyboard overrides need no cleanup — they live only in the running
process. Quit GlazeWM and every shortcut returns to its Windows default.
