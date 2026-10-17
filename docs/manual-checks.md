# i3win manual checks

**Automated (2026-10-17):** `scripts/live-checks.ps1` covers Plan 2 items 1-3 (tiling, focus via Alt+j/Alt+;, keyboard focus follows, workspace hide/show, no typing into hidden windows, no menu on Alt release), 4 (tabbed/stacked/split/floating toggles; not tab clicks or dragging), 5 (kill; not the save prompt), 8, 9, 13; Plan 3 items 8 (one status process across reloads, none after exit/kill); Plan 4 items 1-5, 12. Item 10 (panic recovery) was verified by hand. Still manual: everything visual (colors, title bars, bar contents, DPI scaling, clicks, scrolling), multi-monitor, hung apps, Ctrl+C, and the remaining Plan 3/4 items.

## Plan 2

Prep: `cargo build --workspace` (required: it also builds i3win-status.exe, which the default config uses), close or minimize windows you don't want tiled, run `target\debug\i3win.exe` from a console.

1. `start notepad` x3 -> they tile side by side across the work area (taskbar not covered). Each has a title bar (focused #285577, others #222222) and a 2 px border.
2. Alt+j / Alt+; move focus and keyboard focus really follows (type into each). Alt+Shift+j swaps. Alt+v then Alt+Return opens a terminal below the focused window. Press Alt+j quickly ~10 times: focus must never get stuck.
3. Alt+2 -> Notepads and their decorations vanish; Alt+1 -> they're back in the same layout. Start menu / Notepad menu bar must not open on Alt release. No title bar shows urgent red afterwards. On empty workspace 2, typing must not land in a hidden Notepad.
4. Alt+w -> tab row with 3 tabs; clicking a tab switches. Alt+s -> stacked. Alt+e -> split. Alt+Shift+space -> Notepad floats centered, its title bar stays on top and follows it after a drag. Typing updates the title live.
5. Alt+Shift+q closes the focused Notepad; a dirty file's save prompt appears floating.
6. Maximize a tiled Notepad, then open another window -> it un-maximizes into its tile with no gaps.
7. Start i3win with a minimized window present -> no hole in the layout; restoring it later tiles it.
8. Alt+Shift+e exits -> every window is visible again.
9. Switch to empty workspace 2, then `taskkill /IM i3win.exe /F` -> hidden Notepads stay hidden. Start i3win again -> they reappear and are tiled.
10. `$env:I3WIN_PANIC_AFTER_SECS=15; cargo run -p i3win`, open Notepads, switch to workspace 2 within 15 s -> after the panic all windows reappear; %LOCALAPPDATA%\i3win\i3win.log has a panic line.
11. From a console: hide windows by switching workspace, press Ctrl+C -> windows reappear.
12. Hung app: on workspace 1 run an app that freezes ~5 s; switch to workspace 2 and back after the freeze -> the window is still managed and comes back.
13. Smoke test (empty desktop, no i3win running): `cargo test -p i3win --test smoke -- --ignored --test-threads=1`.
14. Known limitation: while an elevated window (e.g. Task Manager) is focused, bindings don't fire and that window can't be moved/hidden.

## Plan 3: bar and status

Build with `cargo build --workspace` (builds i3win-status.exe too).

1. Run target\debug\i3win.exe with the default config -> a black bar at the top of each monitor; tiled windows start right below it. On a 125/150% scaled monitor the bar/font scale and windows still start at the bar's bottom edge.
2. Right side shows `CPU x% | MEM y% | C: n GiB | [BAT p%] | date time`, updating every second, with separators.
3. Alt+3 -> buttons `1 3` with 3 highlighted; click `1` switches; mouse wheel over the bar goes prev/next one workspace per notch; Alt+r shows a red `resize` indicator, Escape removes it.
4. Bind `workspace "my ws"` and `workspace "a\"b"`; clicking their bar buttons switches to exactly those workspaces (no new workspace appears).
5. Alt+f -> only that monitor's bar disappears; Alt+f again -> it's back.
6. Set `position bottom` in the bar block, Alt+Shift+c -> bars move just above the taskbar; tiled windows end at the bar's top edge.
7. Focus Notepad, type; click empty bar space and a status block -> caret and focused title color stay on Notepad.
8. Alt+Shift+c five times -> Task Manager shows exactly one i3win-status.exe. Alt+Shift+e -> none left. Restart, End task on i3win.exe -> i3win-status.exe disappears too.
9. `status_command nosuchcmd` + reload -> red `status_command exited` block, no cmd.exe churn.
10. Click events: a status script printing `{"version":1,"click_events":true}` then `[` and appending stdin to a file -> clicking a block writes `[`, `{"button":1,...}`, then `,{...}` lines; right/middle click give 3/2.
11. A script printing the header, one valid line, then garbage and a half JSON line -> the bar keeps the valid blocks.
12. Very long status text on a narrow monitor -> blocks drop from the left, never over buttons/mode.
13. Unplug/replug a monitor -> its bar disappears/reappears, no orphan bar.
14. Bottom bar: a context menu, tooltip, or always-on-top window over the bar strip stays above the bar for 5+ s with no 1 Hz flicker.
15. `status_command cmd /c dir /s C:\` -> Alt+2/Alt+1 still react instantly, typing elsewhere doesn't lag, newly opened windows still tile.
16. Touchpad two-finger scroll over the bar -> one workspace per gesture step, not a run through all of them.
17. `exec "C:\Program Files\Windows NT\Accessories\wordpad.exe"` bound to a key launches WordPad (quoted path survives).

## Plan 4: IPC and i3win-msg

Build with `cargo build --workspace` (builds i3win-msg.exe). In PowerShell 5.1 escape inner quotes in JSON args: '[\"window\"]'.

1. With i3win running: `target\debug\i3win-msg.exe workspace 4` prints `[{"success":true}]` and switches; `i3win-msg frob` prints a failure and `$LASTEXITCODE` is 2.
2. `i3win-msg -t get_workspaces`, `-t get_outputs`, `-t get_tree`, `-t get_version`, `-t get_binding_modes`, `-t get_config`, `-t get_bar_config`, `-t get_bar_config bar-0`, `-t get_marks` each print valid JSON (pipe to `ConvertFrom-Json` without error).
3. Second console: `i3win-msg -t subscribe -m '[\"window\",\"workspace\"]'`. Opening Notepad prints window "new" then "focus"; Alt+2 prints workspace "init"/"focus"; closing Notepad prints "close" with container.window set.
4. With the subscriber running, `i3win-msg -t send_tick hello` -> the monitor prints `{"first":false,"payload":"hello"}`; a fresh `-t subscribe -m '[\"tick\"]'` first prints `{"first":true,"payload":""}`.
5. Monitor subscribed to shutdown running, Alt+Shift+e -> it prints `{"change":"exit"}` then ends; every window is visible again.
6. Garbage: `$p=New-Object IO.Pipes.NamedPipeClientStream('.','i3win-ipc','InOut'); $p.Connect(2000); $p.Write([byte[]](1..14),0,14); $p.Dispose()` -> log shows a bad-header line; `i3win-msg -t get_version` still answers. Repeat with a header claiming 0xFFFFFFFF bytes -> rejected, server still answers.
7. Send only "i3-ipc" then dispose -> server still answers.
8. `i3win-msg 'workspace "3: ünï \"q\" x"'` then `-t get_workspaces` shows the name unchanged; the bar shows the same name.
9. Bind `exec cmd /k echo %I3SOCK%` -> the console shows `\\.\pipe\i3win-ipc`. Start i3win with `--ipc-pipe \\.\pipe\mine`: plain `i3win-msg -t get_version` fails, the exec'd console's works (I3SOCK), and `i3win-msg -s \\.\pipe\mine -t get_version` works.
10. Polling: run `while(1){i3win-msg -t get_tree > $null; sleep -m 200}` for 30 s -> no title-bar flicker, no lag.
11. Stalled subscriber: start a monitor, enter QuickEdit selection in its console for 60 s while switching workspaces -> WM stays responsive.
12. IPC smoke test (empty desktop, no i3win running): `cargo test -p i3win --test smoke -- --ignored --test-threads=1`. Known issue: if an assertion in `ipc_roundtrip_and_events` fails, the test kills i3win without restoring windows it hid on other workspaces; start `i3win.exe` once and it restores them from `%LOCALAPPDATA%\i3win\hidden.txt`.

## Plan 5: window commands

1. Config `for_window [class=".*"] title_window_icon on` + reload -> each window's title bar shows its icon left of the title.
2. `border pixel 4` on a focused tiled window -> a 4 px border is drawn; `border none` removes it.
3. `hide_edge_borders both` + reload -> borders on edges touching the screen edge are not drawn.
4. `bindcode $mod+38 exec notepad` -> Alt+A opens Notepad on any keyboard layout (e.g. German, French AZERTY).
5. `bindsym --release $mod+x exec notepad` -> Notepad opens on key up, not key down.

## Plan 6: bar markup and layout

1. A status script emitting `markup: "pango"` with `<b>` and colored `<span foreground="...">` renders bold / colored text in the bar.
2. A block with `short_text` shows the short form on a narrow monitor (or when the bar is crowded).
3. `min_width` + `align` on a block: the text is padded to the minimum width and aligned left/center/right as set.
4. `separator_symbol " | "` draws that text between blocks instead of the separator line.
5. `strip_workspace_numbers yes` with `workspace "1: mail"` -> the button reads `mail`.
6. Two `bar {}` blocks stack (same edge: one above the other; different positions: one top, one bottom).
7. `mode hide` -> the bar shows only while Win is held (or on urgency); `mode dock` keeps it visible.
8. `bindsym button3 exec notepad` inside `bar {}` -> right-clicking the bar opens Notepad (and does not trigger the default click action); `bindsym --release button3 ...` fires on button release; `button4`/`button5` override the wheel's workspace prev/next; `button6`/`button7` fire on horizontal wheel left/right.
9. `title_format "<b>%title</b>"` renders bold window titles in title bars.
