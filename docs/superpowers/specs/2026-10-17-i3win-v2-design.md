# i3win v2: closing the gaps with i3

Date: 2026-10-17
Status: awaiting owner approval. The owner asked to "verify whether any features from the original i3 are missing, and implement them", gave two examples ("custom status bar blocks and styling", "resize/move windows with mouse"), saw the gap list below, and said "do the whole list". Everything else is the designer's call and marked **[call]**.

Builds on `docs/superpowers/specs/2026-10-16-i3win-design.md` (v1). Everything in v1 stays true unless this document says otherwise: its constraints, its architecture (pure `i3win-core`, thin Win32 layer, `Effect`s, `arrange()` diffing), its Win32 decisions and its anti-goals.

## Intent

An i3 user should be able to bring their real i3 config, including the mouse habits, bar block and i3status config, and find it all working. v1 covered the keyboard core; v2 covers what v1 parked: the mouse, a configurable status generator and bar styling, and the window commands and config options v1 skipped.

## Goals

1. **Mouse**: move and resize windows with the mouse the i3 way (floating and tiled), focus follows mouse, mouse warping.
2. **Status**: `i3win-status` reads an i3status-style config (modules, order, formats, colors, intervals) or an i3blocks config (custom command blocks, owner's choice); the bar renders every i3bar block field that matters for looks.
3. **Bar**: the i3 bar options v1 skipped, multiple `bar {}` blocks and hide mode.
4. **Defaults match i3** (owner: "verify that the defaults match"): every built-in default and the generated default config follow i3's, see "Defaults".
5. **Window commands and config**: marks, scratchpad, sticky, `border`, `title_format`, edge/smart borders and gaps, runtime gaps, workspace rename, and the remaining small config options and command forms.
6. Every new piece of logic that can live in `i3win-core` does, with unit tests, as in v1.

## Compatibility rules [call]

- The tree, `Wm`, `Effect`, `Arrangement` and config structs may be extended and refactored freely; v1 unit tests may be updated when a v2 behavior intentionally changes them, never deleted to make a change pass.
- IPC JSON stays i3-shaped: fields are only added, never renamed or removed.
- `hidden.txt` stays `hwnd pid` per line, so a v2 start restores windows hidden by a crashed v1 (and vice versa).
- Every v1 config stays valid with the same meaning, except `focus_follows_mouse` (see Mouse).
- New dependencies are allowed: `chrono-tz` (DST-correct `timezone`), extra `windows` 0.58 features (Core Audio, IP Helper, WLAN API). The `windows` pin stays 0.58.

## Non-goals (v2)

- Saving layout across `restart`, `i3-save-tree`/`append_layout`, a built-in launcher (still out from v1). **[call]**
- Replacing Explorer or hiding the taskbar (v1 non-goal stands). **[call]**
- i3status modules that have no meaning on Windows: `load`, `run_watch` (pidfiles), `ipv6`, `cpu_temperature` (needs admin WMI on most machines). An unknown module shows a red `unsupported: NAME` block, it never crashes. **[call]**
- A tray area. Owner: "leave out tray"; the Windows taskbar already shows those icons. `tray_output` and `tray_padding` are parsed and ignored without a warning, so i3 configs load cleanly.
- Full Pango. Markup supports the subset listed under "Markup". **[call]**

## Anti-goals (in addition to v1's)

- Mouse lag. The low-level mouse hook does what the keyboard hook does: decide, queue, post, return within microseconds. It never calls into other processes and never blocks.
- Swallowing clicks the user meant for an app. The mouse hook swallows a button only when the `floating_modifier` is held over a managed window (or the press lands on our own windows).
- Focus-follows-mouse closing menus, popups or the Start menu. Hover never steals focus while a mouse button is down, while the foreground window is not managed (menus, popups, Start, dialogs of unmanaged windows), or during a drag.
- Scratchpad or sticky windows left hidden after exit or crash. They go through the same hide tracking and `hidden.txt` as workspace hides.

## Mouse

### Hook [call]

`WH_MOUSE_LL` installed on the main thread next to the keyboard hook, same rules: thread-local matcher, no allocation in the common path (moves while no drag is active and focus_follows_mouse off return immediately), queue + `PostMessage`. Mouse moves are coalesced: only the latest position is kept and one wake message is pending at a time (same pattern as v1's `WAKE_PENDING`). The main thread does `WindowFromPoint` / tree lookups, never the hook.

Modifier state comes from the keyboard matcher's held-modifier set (shared on the main thread), not `GetAsyncKeyState`.

### `floating_modifier MOD` (config)

Default config adds `floating_modifier $mod`. While MOD is held:

- **Left-drag on a floating window**: moves it. Live, following the cursor.
- **Right-drag on a floating window**: resizes it from the corner nearest to the press point (i3 behavior). Live. Respects `floating_minimum_size`/`floating_maximum_size`.
- **Right-drag on a tiled window**: resizes the tiled window along the edges nearest to the press point. Live, throttled to at most one re-arrange per 16 ms. **[call]** Algorithm (shared with native border drags below): the press point picks a horizontal edge (left or right half of the window) and a vertical edge (top or bottom half). Each edge is resized independently with i3's directional resize: walk up from the window to the nearest ancestor whose layout is `splith` (for left/right) or `splitv` (for top/bottom) and in which the walked-up child has a sibling on that side; tabbed and stacked ancestors are skipped over; move the boundary between that child and its sibling by the pointer delta, converted to percents of the ancestor's size, clamped so neither side drops below 5%. If no such ancestor exists (the edge is the workspace boundary), that edge doesn't move.
- **Left-drag on a tiled window**: tiling drag (below).

The press and its release are swallowed so the app never sees them. Releasing MOD mid-drag does not end the drag; releasing the button does (i3 behavior).

### Tiling drag (`tiling_drag modifier|titlebar|off`, i3 4.22)

Default `tiling_drag modifier titlebar`. Starts on `$mod`+left-drag on a tiled window, or a plain left-drag on our title bar / tab of a tiled window (when `titlebar` is enabled).

While dragging, a translucent overlay window (`i3win_drop`, layered, click-through, our colors: `client.focused` indicator at 50% alpha) shows where the window will land. On release over:

- **Another tiled window**: the target's rect is split into zones. The center box (middle 30% of width and height) swaps the two windows. Outside it, the rect is split by its two diagonals into four triangles; the triangle containing the pointer names the side (left/right/top/bottom). The dragged window is removed (with v1's removal cleanup) and inserted next to the target on that side: if the target's parent has the matching orientation (`splith` for left/right, `splitv` for top/bottom), it becomes the target's sibling on that side; otherwise the target is wrapped in a new container with the matching orientation holding the target and the dragged window (i3's behavior). If the target sits in a tabbed or stacked container, the dragged window becomes a new tab/stack entry next to the target, whatever the zone. Dropping on a sibling reorders. Dropping on itself, or on a floating or fullscreen window, cancels. **[call]** These rules are a pure function in `i3win-core` with unit tests.
- **A workspace button in a bar**: moves the window to that workspace (not following it).
- **An empty workspace area / anywhere else on an output**: moves the window to that output's visible workspace, appended at the end.
- **Escape** or release over nothing: cancel, nothing changes.

### Native title bars and resize borders [call]

Windows' own move/size loop (dragging an app's native title bar, or its resize border) is detected with `EVENT_SYSTEM_MOVESIZESTART`/`END`:

- **Floating window**: the new rect is kept (as v1).
- **Telling move from resize**: compare the window's DWM frame bounds (`DWMWA_EXTENDED_FRAME_BOUNDS`) at START and at END. If width or height changed by more than 4 physical pixels it was a resize; otherwise a move. **[call]**
- **Tiled window, resized**: at END, each edge that moved by more than 4 px becomes a directional resize (algorithm above) by that edge's delta, so neighbors adjust. Edges on the workspace boundary snap back.
- **Tiled window, moved**: at END, treat the cursor position as a tiling-drag drop (same zones), if `tiling_drag` includes `titlebar`; otherwise snap back as v1.
- Between START and END i3win does not reposition that window (Windows owns it); one full re-arrange runs after END.

No live overlay during native drags (Windows owns the loop), just the result at release.

### Focus follows mouse

`focus_follows_mouse yes|no`. i3's default is `yes`, and an absent line means `yes` as in i3 (owner: "copy i3 exactly"). The generated default config writes `focus_follows_mouse no` explicitly, so new Windows users get click-to-focus, but an i3 config dropped in behaves as on Linux. **[call]** Existing users whose config predates this line get i3's default (`yes`), as the owner chose; the v2 release notes say so, and when the line is absent i3win logs one info line at startup saying hover focus is on. **[call]** Plan 8 tests both settings: with `no`, hovering never changes focus.

Rules: focus changes when the pointer enters a different managed window (or its title bar), after it has rested there 50 ms (avoids focus flicker when crossing windows). **[call]** Never while a button is down, never during a drag, never when the foreground window isn't managed by us, never on a bar. Entering a floating window focuses it; entering a tab title focuses nothing (i3 behavior is to focus only on click for tabs).

### `mouse_warping output|container|none`

Default `output`. When a keyboard command moves focus to a different output, the cursor jumps to the center of the newly focused window (`container`: on every focus change). Warping never triggers focus_follows_mouse (the hover handler ignores the position we set).

### Bar and title bar mouse

- `bindsym [--release] buttonN command` inside `bar {}` (button4/5 wheel, button1-3, button6/7 horizontal). If none are bound, wheel keeps v1's prev/next behavior.
- Clicking a title bar or tab focuses that window (v1). Middle and right clicks on title bars do nothing, as in i3's defaults. **[call]** Wheel over a tab row cycles tabs (i3 behavior).

## Status: `i3win-status`

### Config file

`i3win-status` picks its config in this order (first that exists wins):

1. `-c PATH` (format detected from content: `[name]` sections = i3blocks, `name {` blocks = i3status)
2. `I3BLOCKS_CONFIG` environment variable (i3blocks format)
3. `%USERPROFILE%\.config\i3win\i3blocks.conf`, `%USERPROFILE%\.config\i3blocks\config`, `%USERPROFILE%\.i3blocks.conf` (i3blocks format)
4. `%USERPROFILE%\.config\i3win\i3status.conf` (i3status format)

If none exists, it writes `i3status.conf` with a default that reproduces v1's output (cpu, memory, disk C:, battery, time). i3status syntax:

```
general {
    colors = true
    interval = 5
    color_good = "#00FF00"
    color_degraded = "#FFFF00"
    color_bad = "#FF0000"
    separator = "|"        # only used with output_format=none
    output_format = "i3bar" # i3bar | none (plain text line)
}
order += "cpu_usage"
order += "disk C:"
cpu_usage {
    format = "CPU %usage"
    degraded_threshold = 75
    max_threshold = 90
}
```

Blocks named `MODULE` or `MODULE INSTANCE` (`disk "C:\"`, `battery 0`, `tztime local`, `ethernet _first_`). Values are strings, numbers or `true/false`. Unknown keys: ignored with a warning on stderr. Unknown modules: a red block. Parse errors never crash: the bad block is skipped and a red `config error line N` block is shown. **[call]**

Common per-module keys (i3status): `format`, `format_down`, `color_good/degraded/bad` overrides, `min_width`, `align`, `separator`, `separator_block_width`, `markup` (`pango`|`none`). Each module emits `name` and `instance` in its block so click events can tell blocks apart.

### Modules [call]

| Module | Placeholders | Notes |
|---|---|---|
| `cpu_usage` | `%usage` | thresholds `degraded_threshold`, `max_threshold` (color) |
| `memory` | `%used %free %available %total %percentage_used %percentage_free` | `threshold_degraded`, `threshold_critical` (e.g. `"10%"` or `"1G"` free) |
| `disk PATH` | `%free %used %total %avail %percentage_free %percentage_used` | `prefix_type` binary/decimal, `low_threshold` + `threshold_type` |
| `battery N\|all` | `%status %percentage %remaining %emptytime` | `status_chr/bat/full/unk`, `low_threshold`, `format_down` when no battery |
| `tztime NAME` / `time` | strftime `format` | `timezone` (IANA name via a bundled table or Windows TZ id) **[call: use `chrono-tz`]** |
| `ethernet IF` / `wireless IF` | `%ip %speed` / `%ip %essid %quality` | `IF` = adapter friendly name or `_first_`; `format_down` |
| `volume NAME` | `%volume` | default render endpoint via Core Audio; `format_muted` |
| `path_exists NAME` | `%title %status` | `path` |
| `read_file NAME` | `%title %content` | `path`, `max_characters` |

Each module refreshes on the global `interval` (per-block `interval` overrides it **[call]**, an i3blocks-style extension).

### i3blocks config (custom blocks)

The owner asked for "custom status bar blocks" and chose i3blocks' config format, read by `i3win-status` itself (no separate program), so an i3blocks user can copy their config over. Behavior follows i3blocks 1.5 unless stated.

```
# global properties: defaults for every block below
separator_block_width=15
shell=bash

[cpu_usage]
format=CPU %usage

[weather]
command=curl -s wttr.in/?format=1
interval=600

[disk]
command=$SCRIPT_DIR/disk.sh
DISK=C:
LABEL=HDD

[tztime local]
format=%H:%M
```

- **File syntax**: INI-like. `key=value` lines before the first section are global properties, inherited by every block (a block's own value wins). `[name]` starts a block; `#` comments; blank lines ignored. Duplicate names allowed.
- **Built-in modules from i3blocks.conf** (i3win addition): a section with no `command=` whose name is an i3win-status module (`cpu_usage`, `disk C:`, `tztime local`, ...) shows that module, with the section's keys as the module's settings (same keys as in i3status.conf). A section with neither `command` nor a known module name shows its static `full_text` (i3blocks behavior).
- **Properties** passed to the bar: `full_text`, `short_text`, `label` (prefixed to `full_text`), `color`, `background`, `border`, `border_top/right/bottom/left`, `min_width`, `align`, `urgent`, `name` (= section name), `instance`, `separator`, `separator_block_width`, `markup`.
- **Custom properties as environment**: every property of the block (global and own, including unknown keys like `DISK`, `LABEL`) is set as an environment variable for its command, as i3blocks does. Also `SCRIPT_DIR` = folder of the config file, and `BLOCK_NAME`/`BLOCK_INSTANCE`/`name`/`instance`.
- **Variable expansion** (owner: match i3blocks): i3blocks leaves expansion to `sh`. So with a `shell=` other than `cmd`, i3win-status passes `command` through untouched and the shell expands it. Only with `cmd` (which can't) does i3win-status expand it itself first, following sh's rules: `$VAR` and `${VAR}` from the block's environment, unknown variables expand to empty, nothing inside `'single quotes'` is expanded (the quotes are removed), `\$` is a literal `$`, and `$$` is the process ID of i3win-status (the closest thing to sh's own PID).
- **Shell**: `shell=` property (i3win addition, may be global or per block): `cmd` (default; runs `cmd /S /C "COMMAND"` as v1 `exec`), or any other program, run as `PROGRAM -c "COMMAND"` (e.g. `bash`, `sh`, `pwsh` gets `-Command`). Hidden window. **[call]**
- **`interval`**: N seconds; `once` (run at start only); `repeat` (rerun as soon as it exits); `persist` (keep running, every output line updates the block). No interval = run once at start and on clicks/signals (i3blocks).
- **`format`**: `raw` (default): line 1 `full_text`, line 2 `short_text`, line 3 `color`; or `json`: each line is a JSON object whose keys update the block's properties.
- **Exit codes** (owner: match i3blocks 1.5): 0 normal; 33 = `urgent`; any other nonzero is an error: the block shows i3blocks' error text (`bad exit code N`) in red (`#FF0000`) instead of the output. The planner checks the exact i3blocks 1.5 wording and color from its source (`block.c`) and copies them.
- **No timeout** (owner: match i3blocks): a command may run as long as it likes; the block keeps its last output, and a block is never started again while its previous run is still going (clicks and signals that arrive meanwhile are dropped, as in i3blocks). On reload or exit the whole process tree is killed (v1 Job Object).
- **Clicks**: clicking a non-persist block reruns its command with the click in the environment, in both naming styles: `button`, `x`, `y`, `relative_x`, `relative_y`, `width`, `height`, `modifiers` (1.5) and `BLOCK_BUTTON`, `BLOCK_X`, `BLOCK_Y` (legacy). For `persist` blocks the click is written to the running process's stdin as one i3bar click JSON line instead.
- **Signals**: `signal=N` property. Windows has no Unix signals, so `i3win-status --signal N` replaces `pkill -RTMIN+N i3blocks`: every running i3win-status reruns its blocks with that signal (persist blocks get nothing, as in i3blocks). `i3win-status --refresh-all` reruns every non-persist block (replaces `pkill -USR1 i3blocks`). Delivery: each running i3win-status owns a message-only window of class `i3win_status_sig`; the `--signal`/`--refresh-all` invocation finds them all with `FindWindowExW(HWND_MESSAGE, ...)` and posts a message, so every bar's instance refreshes. **[call]**
- **Concurrency**: each running command has its own reader thread; outputs go to the single writer thread that prints i3bar lines (at most one line per 50 ms, coalesced). **[call]**
- **Known difference**: commands inside scripts that only exist on Linux won't run on Windows; the config file itself carries over.

## Bar

### Block rendering (i3bar protocol fields)

The bar draws: `full_text`, `short_text` (all blocks switch to short text when the full line doesn't fit, before dropping blocks from the left as v1 does), `color`, `background`, `border` + `border_top/right/bottom/left` widths (default 1 when `border` is set), `min_width` (pixels, or a string whose width is measured), `align` left/center/right within `min_width`, `urgent` (urgent colors), `separator`, `separator_block_width`, `markup: "pango"`.

### Markup [call]

Pango subset, shared by bar blocks, `title_format` and mode names: `<b> <i> <u> <s> <small> <big> <sup> <sub> <tt>`, `<span>` with `foreground`/`fgcolor`/`color`, `background`/`bgcolor`, `weight` (`bold|normal|light|NNN`), `style` (`italic|normal`), `underline` (`single|none`), `size` (`small|large|x-large|NNpt` or a percentage), `font_family`/`face`, `rise` ignored. Entities `&amp; &lt; &gt; &quot; &apos; &#NN;`. Unknown tags are dropped (their text kept); malformed markup renders as plain text. Parser in `i3win-core`, drawing in GDI by text runs.

### Bar config additions

Inside `bar {}`:

- `id NAME` (default `bar-0`, `bar-1`, ... in file order). Multiple `bar {}` blocks are allowed; each makes its own set of bar windows and runs its own `status_command`.
- `output NAME` (repeatable; `primary`; `nonprimary`): which monitors get this bar. Default: all.
- `mode dock|hide|invisible`, `hidden_state hide|show`, `modifier MOD|none` (default `Mod4`, as i3). In `hide` mode the bar is shown while the modifier is held, while a workspace is urgent, or while `hidden_state show`; it overlaps windows (no reserved space) as in i3.
- `separator_symbol "|"`, `strip_workspace_numbers yes|no`, `strip_workspace_name yes|no`, `binding_mode_indicator yes|no`, `workspace_min_width N`, `padding` (i3 4.22 CSS-like 1-4 values). `tray_output`/`tray_padding`: accepted and ignored (no tray).
- `colors {}` adds `focused_background`, `focused_statusline`, `focused_separator` (used on the bar of the focused output).
- `bindsym [--release] buttonN command` (see Mouse).

Plan 6 exposes a hit-test (`bar window + point → workspace name | block | none`) that Plan 8's tiling drag uses. A bar hidden by `mode hide` is not a drop target. **[call]**

Commands: `bar mode dock|hide|invisible|toggle [ID]`, `bar hidden_state hide|show|toggle [ID]`. IPC: `GET_BAR_CONFIG` lists all ids and returns full configs; `barconfig_update` event on change.

## Window commands and config

### Marks

- `mark [--add|--replace] [--toggle] NAME`, `unmark [NAME]`. Marks are unique across the tree: setting a mark on one window removes it from any other.
- Criteria `con_mark="regex"`, and `move [container|window] to mark NAME`, `swap container with mark NAME | con_id N | id HWND`.
- `show_marks yes|no` (default yes): title bars show `[mark]` after the title.
- `GET_MARKS` returns the real list; tree JSON nodes have `"marks"`; window event `mark`.

### Scratchpad

- `move scratchpad`: the window becomes floating and goes to the hidden `__i3_scratch` workspace.
- `scratchpad show`: shows the most recently hidden scratchpad window centered on the focused workspace (floating); if a scratchpad window is visible and focused, hides it again; if one is visible but unfocused, focuses it. With criteria (`[class="x"] scratchpad show`), toggles the matching window(s). Same as i3.
- Hidden via the v1 hide path (tracked, `hidden.txt`), so exit and crash restore them.
- Tree JSON includes the `__i3_scratch` workspace; `scratchpad_state` on nodes.

### Sticky

`sticky enable|disable|toggle`: a sticky floating window stays visible on whichever workspace is shown on its output. Tiled windows ignore it (as i3).

### Borders and titles

- `border normal [N] | pixel [N] | none | toggle [N]` command (v1 had only the config defaults).
- `title_format FORMAT` command and `for_window [...] title_format`: placeholders `%title %class %instance %machine` (`%machine` = computer name), markup allowed.
- `title_window_icon yes|no|padding Npx`: draws the window's small icon in the title bar. Icon from `GetClassLongPtrW(GCLP_HICONSM)` (non-blocking) and, asynchronously, `WM_GETICON` via `SendMessageCallbackW`, never a blocking send. **[call]**
- `hide_edge_borders none|vertical|horizontal|both|smart|smart_no_gaps`.
- `smart_borders on|no_gaps|off`, `smart_gaps on|off|inverse_outer`.

### Gaps

Config: `gaps inner|outer|horizontal|vertical|top|right|bottom|left N`, and `workspace NAME gaps inner|outer|... N`. Command: `gaps inner|outer|horizontal|vertical|top|right|bottom|left current|all set|plus|minus|toggle N`. Semantics as i3 4.22.

### Workspaces

- `rename workspace [OLD] to NEW` (and `rename workspace to NEW` for the focused one). Emits workspace `rename` event.
- `workspace_auto_back_and_forth yes|no`.
- `workspace --no-auto-back-and-forth NAME` flag.

### Focus and windows

- `focus_wrapping yes|no|force|workspace` (default `yes`).
- `focus next|prev [sibling]`.
- `focus_on_window_activation smart|urgent|focus|none` (what happens when an app calls `SetForegroundWindow`; default `smart`: focus if on a visible workspace, otherwise mark urgent, which is v1's behavior).
- `popup_during_fullscreen smart|ignore|leave_fullscreen` (default `smart`: a new floating dialog of the fullscreen app shows on top; other new windows don't steal focus).
- `no_focus [criteria]`: matching new windows don't get focus when they appear.
- `floating_minimum_size WxH`, `floating_maximum_size WxH` (`-1` = unlimited, `0` = default).
- `move position X [px|ppt] Y [px|ppt]`, `move [absolute] position center`, `move position mouse`, `move container to output left|right|up|down|NAME`.
- `resize set [width] W [px|ppt] [height] H [px|ppt]`.
- `layout toggle [split|all|LAYOUT...]` (cycle through a given list).
- `urgent` criterion and `[urgent=latest|oldest]`, `[con_id=N]`, `[id=HWND]`, `[workspace="regex"]`, `[floating]`, `[tiling]` criteria.
- `[con_id=__focused__]` / `__focused__` values for class/instance/title/workspace as in i3.

### Bindings

- `bindsym --release COMBO cmd`: the press is swallowed, the command runs on release of the main key.
- `bindcode [--release] CODE cmd`: CODE is the X11 keycode i3 configs use (evdev code + 8). i3win maps it from the Windows scan code (`KBDLLHOOKSTRUCT.scanCode`, plus the extended flag) with a fixed table in `i3win-core` covering the full PC 105-key layout. **[call]** So `bindcode 38` (A on X11) fires on the physical A key on any layout. The table includes extended keys (right Ctrl/Alt, arrows, Insert/Delete/Home/End/PgUp/PgDn, numpad Enter and `/`, Pause, PrintScreen, Win keys). A code with no table entry is a config warning and the line is skipped.

## Defaults

Every default below is i3's (4.22). v1 values that differ are changed in Plan 5 and listed in the release notes.

| Setting | i3 default | v1 had |
|---|---|---|
| bar `position` | `bottom` | `top`: **change** (and drop v1's "[call]" for top) |
| `font` with no `font` line | i3's `fixed` → i3win uses `monospace 8` | `Segoe UI 9`: **change** |
| bar `mode` / `hidden_state` / `modifier` | `dock` / `hide` / `Mod4` | (new) |
| bar `workspace_buttons`, `binding_mode_indicator` | `yes`, `yes` | `yes`, (new) |
| bar `strip_workspace_numbers` / `_name` | `no` | (new) |
| bar `separator_symbol` | none (a 1 px vertical line) | line: same |
| bar colors, `client.*` colors | i3's | already identical |
| `default_border` / `default_floating_border` | `normal 2` | same |
| `workspace_layout` | `default` | same |
| `focus_follows_mouse` | `yes` | (new) |
| `mouse_warping` | `output` | (new) |
| `focus_wrapping` | `yes` | (new) |
| `workspace_auto_back_and_forth` | `no` | (new) |
| `popup_during_fullscreen` | `smart` | (new) |
| `focus_on_window_activation` | `smart` | v1 behavior = smart |
| `show_marks` | `yes` | (new) |
| `hide_edge_borders` / `smart_borders` / `smart_gaps` | `none` / off / off | (new) |
| `floating_modifier` | none unless set (default config sets `$mod`) | (new) |
| `tiling_drag` | `modifier titlebar` | (new) |
| `floating_minimum_size` / `maximum` | 75x50 / unlimited | (new) |
| gaps | 0 | same |

**Font names** [call]: Pango generic families map to Windows fonts: `monospace` → Consolas, `sans`/`sans-serif` → Segoe UI, `serif` → Times New Roman. Other names are passed to GDI as is.

**Generated default config** follows i3's shipped `/etc/i3/config` line by line, with these Windows substitutions only: terminal `wt` (falling back to `cmd`) for `i3-sensible-terminal`; the Run dialog for `dmenu_run`; `i3win-status` for `i3status`; the X-only lines (`xss-lock`, `nm-applet`, `pactl` volume keys, which Windows handles itself) are dropped. So it gains, relative to v1's: `font pango:monospace 8`, `floating_modifier $mod`, `tiling_drag modifier titlebar`, `focus_follows_mouse no` (the one deliberate addition, see Mouse), the scratchpad bindings `$mod+Shift+minus move scratchpad` and `$mod+minus scratchpad show`, and a `bar {}` with no `position` line (so bottom). `$mod+Shift+e` keeps exiting directly: i3 asks through `i3-nagbar`, and i3win has no nagbar. **[call]** An existing config file is never rewritten.

## IPC additions

GET_MARKS real data; tree JSON `marks`, `sticky`, `scratchpad_state`, `window_icon_padding`, `title_format`; events `window::mark`, `window::title` (title changes), `window::move`, `window::floating`, `window::urgent`, `workspace::rename`, `barconfig_update`. GET_BAR_CONFIG full shape for all ids.

## Plan split [call]

Four plans, each shippable alone, in this order:

1. **Plan 5: window commands, config and defaults** (mostly `i3win-core`): the Defaults section, marks, scratchpad, sticky, border/title_format/icons, edge/smart borders and gaps, runtime gaps, rename, focus options, criteria additions, move/resize forms, `--release`/`bindcode`, IPC additions.
2. **Plan 6: bar and markup**: markup parser, block field rendering, short_text, bar config additions, multiple bars, hide mode, bar button bindings, bar commands.
3. **Plan 7: i3win-status**: i3status and i3blocks config parsers, config discovery, modules, command blocks (env, expansion, shell, intervals, formats, clicks, signals).
4. **Plan 8: mouse**: mouse hook, floating_modifier move/resize, tiling drag with overlay, native move/size handling, focus_follows_mouse, mouse_warping.

## Owner decisions

- Tray: left out.
- `focus_follows_mouse`: copy i3 exactly (absent line = `yes`).
- Defaults: match i3's.
- Custom status blocks: built into `i3win-status`, using the i3blocks config format and its features (custom properties as environment, `$SCRIPT_DIR`, clicks in both naming styles, persist clicks on stdin, per-side borders, i3blocks config locations, signal and refresh-all replacements), plus a `shell=` option. Exit codes, (no) timeout and variable expansion match i3blocks exactly.

## What's left to the builder

Internal structure of new files, how the GDI text-run renderer caches fonts, exact overlay alpha and colors beyond what's stated, log wording, the module refresh scheduler's internals, and the X11 keycode table's source (any standard evdev table).
