# i3win manual checks (Plan 2)

Prep: `cargo build -p i3win`, close or minimize windows you don't want tiled, run `target\debug\i3win.exe` from a console.

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
