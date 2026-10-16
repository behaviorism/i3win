# i3win manual checks (Plan 2)

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
