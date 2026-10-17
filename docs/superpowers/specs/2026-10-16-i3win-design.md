# i3win: an i3-style tiling window manager for Windows

Date: 2026-10-16
Status: design approved by owner skipping the Q&A ("skip"); every decision below not quoted from the original request is the designer's call and marked **[call]**.

## Intent

The owner's request: "a windows version of the i3 window manager, including the graphical components (bar, windows titles, etc.)", implemented in Rust, with the coding done by Sonnet to save credits.

Success = someone who uses i3 on Linux can drop a familiar i3 config onto Windows 10, start `i3win.exe`, and work the way they do in i3: keyboard-driven tree tiling, numbered workspaces, i3-style title bars and tabs, an i3bar-style bar at the edge of each monitor, and `i3-msg`-style scripting.

Because Sonnet does the building, the plans must be split into small, fully specified tasks, and as much logic as possible goes into a pure, unit-testable core crate.

## Goals

1. **Tree tiling, the way i3 does it**: outputs → workspaces → containers → windows. Layouts `splith`, `splitv`, `tabbed`, `stacking`. Floating and fullscreen windows.
2. **Workspaces**: named or numbered (1–10 bound by default), with each monitor showing one workspace. Empty workspaces disappear when you leave them. `back_and_forth`, `next`/`prev`, moving workspaces between monitors.
3. **i3 config subset** (see "Config" below), read from `%USERPROFILE%\.config\i3win\config`. If that file is missing, the WM writes a default config there.
4. **Global keybindings** with binding modes (for example a `resize` mode), through a low-level keyboard hook. Bound keys are swallowed so apps never see them.
5. **i3 command language subset** (see "Commands"), shared by keybindings, IPC and the bar.
6. **Decorations**: i3-style title bars, borders, and tabbed/stacked header rows drawn in our own windows, using the colors from `client.*` in the config.
7. **Bar**: one per monitor showing workspace buttons (click to switch), the binding-mode indicator, and a status area fed by `status_command` (i3bar JSON protocol or plain text). Ships with a companion `i3win-status.exe` that shows CPU, memory, disk, battery and date/time.
8. **IPC**: the i3 IPC wire format over the named pipe `\\.\pipe\i3win-ipc`, plus `i3win-msg.exe` (an `i3-msg` clone).
9. **Never strand windows**: on exit, crash, or Ctrl+C, every window we hid comes back. The next start also restores any hidden windows a crashed previous run left behind.

## Non-goals (v1)

- Replacing Explorer, or hiding or replacing the Windows taskbar. i3win runs alongside Explorer the way GlazeWM and komorebi do. **[call]**
- Removing apps' own (native) title bars. Our title bars sit above the app's window. **[call]**
- Mouse features: `floating_modifier` dragging, `focus_follows_mouse`, resizing tiled windows with the mouse. Mouse-moving a floating window works through its native title bar. If you drag or resize a tiled window, it snaps back. **[call]**
- Saving layout across `restart`. `restart` means reload the config and lay windows out again. **[call]**
- Marks, scratchpad, `i3-save-tree`/`append_layout`, `--release` bindings, `bindcode`, `hide_edge_borders`, `smart_gaps`, `title_format`, a tray area, a built-in app launcher. **[call]**
- Windows 11-only APIs. Target is Windows 10 19045 and newer. **[call]**

## Anti-goals (failure even if it "works")

- Windows left hidden after i3win exits or crashes.
- Keyboard input lag. The keyboard hook must return within microseconds and never block. If it blocks, Windows silently removes it.
- The Start menu (or an app's menu bar) popping open after every Win- or Alt-based binding.
- A crash on any malformed config line. Unknown or bad lines are logged and skipped.
- Freezing because one app hangs. Never make a synchronous cross-process call that can block on a hung window.

## Constraints

- Rust stable (2021 edition), MSVC toolchain. The machine has VS Build Tools 2019; Rust gets installed via `winget install Rustlang.Rustup`.
- Win32 bindings via the `windows` crate, version `0.58`, pinned. GDI for drawing: it is simpler and plenty for flat text bars. **[call]** (The original idea of Direct2D was dropped.)
- Pure logic lives in `i3win-core` with no `windows` dependency, so it unit-tests on any machine.
- Single-threaded UI: the main thread owns the message loop, the WinEvent hook, the keyboard hook, the decorations and the bars. IPC and status-command readers run on worker threads that send messages to the main thread.
- Per-monitor DPI aware (v2). Sizes in the core are physical pixels; each output carries a `scale` factor.

## Architecture

Cargo workspace `i3win/` with four crates:

| Crate | Kind | Responsibility |
|---|---|---|
| `i3win-core` | lib | tree, commands, config, keys, layout geometry, bar model, i3bar protocol parser, IPC framing and JSON. Pure. |
| `i3win` | bin | Win32 platform layer: hooks, window control, decorations, bars, status process, IPC server, the main loop |
| `i3win-msg` | bin | IPC client (`i3-msg` clone) |
| `i3win-status` | bin | i3status-like generator that prints the i3bar JSON protocol |

Design: a functional core with a thin platform layer around it. The platform turns Win32 events into calls on `Wm`. `Wm` changes the tree and queues `Effect`s for one-off actions (exec, close, focus, exit, reload, IPC events). After each event, the platform applies the effects, asks the core for `Layout` (where every window goes, which are hidden, what to draw), and applies only what changed since the last layout.

### Win32 decisions [call]

- **Hiding windows on other workspaces**: `ShowWindow(SW_HIDE)`. Every hide we cause is counted in an "expected hides" map so the resulting `EVENT_OBJECT_HIDE` isn't read as the window going away. Hidden HWNDs are written to `%LOCALAPPDATA%\i3win\hidden.txt` so a crashed run can be recovered.
- **Which windows get managed**: visible, top-level, unowned, has `WS_CAPTION`, not `WS_EX_TOOLWINDOW`, not DWM-cloaked, not from our own process, non-empty title, and window class not in the ignore list (`Shell_TrayWnd`, `Shell_SecondaryTrayWnd`, `Progman`, `WorkerW`, `Windows.UI.Core.CoreWindow`, `ApplicationFrameWindow` while it is cloaked). Windows with `WS_EX_DLGMODALFRAME`, or without `WS_THICKFRAME`, start floating.
- **Invisible resize borders**: when placing a window, the target visible rect is grown by the difference between `GetWindowRect` and `DWMWA_EXTENDED_FRAME_BOUNDS`.
- **Positioning**: `SetWindowPos(... SWP_NOZORDER|SWP_NOACTIVATE|SWP_ASYNCWINDOWPOS)`. A maximized window gets `ShowWindow(SW_RESTORE)` before it is placed.
- **Focus**: send an unassigned virtual key (`VK 0xE8`) down and up with `SendInput`, then call `SetForegroundWindow`. The same dummy key is sent when a swallowed Win or Alt combination ends, so the Start menu or app menu bar doesn't open.
- **Minimize**: a minimized window leaves the tree. When restored, it is re-inserted on the focused workspace.
- **Elevated (admin) windows** can't be moved by a non-elevated i3win. Failures are logged and the window stays managed (documented limitation).
- **Our own windows** (decorations, bars, the hidden message window) use class names that start with `i3win_` and are excluded from management by the own-process check.

### Mapping i3 window properties [call]

- `class` = the process executable's name without `.exe` (for example `firefox`)
- `instance` = the Win32 window class name
- `title` = the window title

Criteria are regular expressions (`regex` crate), as in i3.

## Config (supported subset)

Lines are trimmed. `#` starts a comment (only at the beginning of a line). A trailing `\` continues the line. Directives:

- `set $name value`. Variables expand everywhere afterward, longest name first.
- `bindsym [--whole-window] COMBO command`. `--whole-window` is ignored. Modifiers: `Mod1` = Alt, `Mod4` = Win, `Shift`, `Control`/`Ctrl`.
- `mode "name" { bindsym ... }`
- `exec [--no-startup-id] cmd` and `exec_always [--no-startup-id] cmd`
- `font pango:Family Name Size` or `font Family Size`
- `default_border normal [N] | pixel N | none` (default `normal 2`) and `default_floating_border` (same forms)
- `workspace_layout default|stacking|tabbed`
- `for_window [criteria] command`
- `assign [criteria] [→] [workspace] [number] NAME`
- `workspace NAME output OUTPUT...`
- `gaps inner N`, `gaps outer N`
- `client.focused|focused_inactive|unfocused|urgent border background text indicator [child_border]`
- `bar { position top|bottom; status_command cmd; font ...; height N; workspace_buttons yes|no; colors { background, statusline, separator, focused_workspace b bg t, active_workspace, inactive_workspace, urgent_workspace, binding_mode } }`
- Anything else: log a warning and skip it.

Default bar position is `bottom`, as in i3 (v2 spec).

Default config: the i3 default config, adapted. `$mod` is `Mod1` (Alt), because Win-key combinations collide with the OS and Win+L can't be intercepted at all. **[call]** `$mod+Return` runs `exec wt`, falling back to `cmd` if Windows Terminal isn't on PATH. `$mod+d` opens the Windows Run dialog.

## Commands (supported subset)

Commands can be chained with `;` or `,`, and can carry a criteria prefix `[class="..." title="..." instance="..."]`.

`exec [--no-startup-id] CMD` · `focus left|right|up|down|parent|child|mode_toggle|floating|tiling` · `focus output left|right|up|down|NAME` · `move left|right|up|down [N px]` · `move [container|window] to workspace NAME|number N|next|prev|back_and_forth` · `move workspace to output left|right|up|down|NAME` · `workspace NAME|number N|next|prev|back_and_forth` · `split h|v|horizontal|vertical|toggle` · `layout stacking|tabbed|splith|splitv|default|toggle split|toggle all` · `fullscreen [toggle|enable|disable]` · `floating toggle|enable|disable` · `kill` · `resize grow|shrink width|height N [px] [or M ppt]` · `mode NAME` · `reload` · `restart` · `exit` · `nop`

Running a command returns i3-style results: `[{"success":true}]`, or `{"success":false,"error":"..."}` per command.

## Behavior details the builder must not reinvent

The plans carry the exact algorithms for: inserting a window, removing one and cleaning up, focus in a direction, move in a direction, split, changing layout, resize, workspace switching, and layout geometry (gaps, title height, tabbed/stacked headers, borders).

## IPC

The wire format is the i3 one: `"i3-ipc"` magic, then a u32 payload length, then a u32 message type, both little-endian, then the JSON payload.

- Requests: RUN_COMMAND 0, GET_WORKSPACES 1, SUBSCRIBE 2, GET_OUTPUTS 3, GET_TREE 4, GET_MARKS 5 (always `[]`), GET_BAR_CONFIG 6, GET_VERSION 7, GET_BINDING_MODES 8, GET_CONFIG 9, SEND_TICK 10
- Events: workspace, output, mode, window, binding, shutdown, tick. JSON shapes follow the i3 docs for the fields we have.

## What's left to the builder

Internal helper structure inside each file, log wording, exact GDI color and brush caching, and the numbering of private window messages.
