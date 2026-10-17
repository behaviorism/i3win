# i3win v2 build: rulings and parked items

Every decision the controller made on the owner's behalf while executing Plans 5-8 (subagent-driven), in order, plus parked findings. Each ruling states what it costs if wrong.

## plan5-window-commands

- Task 2: Ruling: test_wm() now sets font 9pt so v1 20px-title geometry tests keep their meaning — test helper only, product default is monospace 8 — cost if wrong: tests don't exercise the 8pt default geometry.
- Task 5: Ruling: `gaps ... all` also clears affected sides from config.workspace_gaps (in-memory) — spec says i3 4.22 semantics where `all` applies to every workspace; plan text only named node overrides — cost if wrong: `all` overrides per-workspace config gaps until reload.
- Ruling: implementer commits may carry their own harness attribution trailer (e.g. Sonnet 5.5) instead of the plan-stated Opus 5.5 — the subagent harness supplies the accurate model attribution — cost if wrong: inconsistent trailers in history.
- Task 6: Ruling: assign rejects criteria beyond class/instance/title and for_window rejects `urgent` (warn + skip) — matching would otherwise silently over-match or drop filters — cost if wrong: some i3 configs using those keys in assign lose those lines.
- Task 6: Ruling: urgent_since stamped only on becoming urgent (pulled minor into fix round) — i3 semantics, brief wording — cost if wrong: none.
- Task 8: Ruling: Fresh scratchpad windows show at 50% output width x 75% height, centered (i3 4.22) — brief said only "centered" — cost if wrong: different initial size.
- Task 8: Ruling: scratchpad show pulls a scratch window visible on another output to the focused workspace first (i3 4.22 step the brief omitted) — cost if wrong: extra behavior.
- Task 8: Ruling: workspace names starting with "__" rejected in commands/config (i3 behavior) — cost if wrong: users can't name workspaces "__x".
- Task 12: Ruling: non-focusing manage on the focused workspace skips focus calls (attach appends to focus lists) instead of the plan's set_focused_within, which would steal focus there — cost if wrong: none expected; hidden-ws path unchanged.
- Task 14: Ruling: layout toggle LIST 'split' from tabbed/stacked yields splith (no per-container last_split_layout memory, same as v1 toggle split) — i3 remembers the last split per container — cost if wrong: a container that was splitv returns as splith after toggling through tabbed.
- Ruling: live desktop checks (scripts/live-checks.ps1) deferred to after Plan 8 — the user is actively at the PC (answering questions) and the script minimizes windows and types keys — cost if wrong: platform bugs found later.
- Final: parked — floating window moved to a workspace mark on another output keeps absolute rect (not shifted like move to workspace) — Ruling: real but minor, tree stays valid; follow-up: shift_floating in move_to_mark — cost if wrong: window appears off its new output after cross-output mark move.

## plan6-bar-markup

- | T2 -> T4, T6, Plan 7 | T2 `Block` new fields, `MinWidth`, `Click`, `ClickWriter::encode(&Click)`, `MainMsg::StatusClick{bar, click}` | consistent; Plan 7 needs `Click: Deserialize` — Ruling: T2 derives Serialize+Deserialize with serde(default) on Click now |
- Ruling: Click derives Serialize + Deserialize (#[serde(default)] fields) in Plan 6 Task 2, so Plan 7 can parse stdin clicks — cost if wrong: none.
- Ruling: clicks/wheel (buttons 1-7) over a status block go to the status process when it asked for click events, before bar bindings/defaults (i3bar behavior; spec goal: i3 bar configs work) — plan Task 8 said bindings win — cost if wrong: a `bindsym button4` no longer fires over blocks of a click-aware status command.
- Ruling: final fix wave also takes robustness items: lenient float numeric block fields, bounded font cache, dedupe bar ids — cheap and prevent bar freeze / GDI exhaustion — cost if wrong: none.
- Ruling: button releases over a click-aware status block are swallowed (not sent, no --release binding) — i3bar sends block clicks on press only — cost if wrong: --release bar bindings don't fire over such blocks.
- Final: parked — unbound horizontal wheel (buttons 6/7) has no default action (i3bar switches workspaces) — Ruling: not required by plan/spec, minor — cost if wrong: sideways scroll on the bar does nothing.
- Final: parked — bar.rs ~660 lines — Ruling: acceptable for now; split candidate later — cost if wrong: maintainability.

## plan7-status

- Ruling: keep the existing `#[derive(Default)]` on core `Block` (other code relies on it) and add `Block::new(full_text: impl Into<String>) -> Block` in core with i3 defaults (separator: true); all i3win-status code builds blocks via `Block::new` (make_block, static_block, error blocks), never `Block::default()` — plan T7 asked for a manual Default impl, which would silently change existing behavior — cost if wrong: none.
- Task 4: Ruling: Cargo.lock committed with the chrono-tz addition — required for reproducible builds — cost if wrong: none. Duplicate Win32 readers in modules.rs vs mods/ until Task 7 removes modules.rs.
- Task 4: parked-minor: disk low_threshold for bytes_free/bytes_avail treated as GiB — i3status unit not verified — cost if wrong: thresholds off by unit.
- Task 7: Ruling: command blocks finish on process exit (waiter thread + short drain), not stdout EOF — plan said EOF; on Windows children inherit stdout so "click to open app" blocks froze and dropped clicks — cost if wrong: output written >150 ms after exit is dropped.
- Task 7: Ruling: cap block output (64 KiB/line, 1 MiB/run) and exit on stdin EOF — restores v1 line cap, avoids lingering process — cost if wrong: none.
- Ruling: decode non-UTF-8 child output as OEM code page (both i3win-status and the bar's plain status reader) — Windows console tools emit OEM — cost if wrong: a genuinely Latin-1 script could be mis-decoded.
- Final: parked — blocks_json only shortens full_text/short_text; huge name/instance/color from a JSON block could still exceed the bar's line cap — Ruling: pathological, minor — cost if wrong: such a script freezes bar updates.

## plan8-mouse

- Task 3: Ruling: swallowed modifier+click with Alt/Win sends the v1 dummy key to mask menu activation — anti-goal (no Start menu popping), plan omitted it — cost if wrong: none.
- Task 3: Ruling: Snapshot.foreground_managed gates swallowing (unmanaged foreground → presses pass) — Review Focus 1, no cross-process calls allowed in hook — cost if wrong: modifier-drag doesn't start while an unmanaged window (e.g. a popup) is foreground.
- Task 3: Ruling: MouseMatcher::force_release for resync when a button-up was missed (App checks GetAsyncKeyState during drags) — cost if wrong: none.
- Task 4: Ruling: tiled Left-drag gating = titlebar setting for title-bar drags, modifier setting otherwise (i3 semantics; brief literally said OR) — cost if wrong: none.
- Ruling: a native move/size loop that starts with the cursor outside the frame's resize border is a MOVE (Snap/maximize at END doesn't count as resize); IsZoomed at END also = move — spec [call] said compare frame bounds only — cost if wrong: a resize started within 8 px of an edge but outside the border band is treated as a move.
- Ruling: hover blocked while the foreground thread is in menu/move-size mode or has capture (GetGUIThreadInfo on main thread) — anti-goal hover never closes menus — cost if wrong: hover pauses while an app holds mouse capture.
- Ruling: warping only when the pointer is on a different output (Output mode) / outside the focused window (Container mode); bar clicks never warp — matches i3 behaviour — cost if wrong: none.
- Ruling: live swap check asserts left.L >= other.R(current) - 16 (brief's 'former R' wording was wrong) — cost if wrong: none.

## Deferred minor findings

Per-task reviews also logged minor findings judged safe to defer; the final whole-plan reviews triaged them and many were fixed in each plan's final fix wave (commits 78592b5, 3c60945, 805c3a7, 29a6154). This is the raw log as recorded; an item here may since have been fixed.

### plan5-window-commands

- Task 1: minor (deferred): parse_modifiers duplicates keys::parse_combo's modifier table (config.rs:177).
- Task 1: minor (deferred): parse_bool accepts broad synonyms (on/active/enable) beyond yes/no.
- Task 1: minor (deferred): no RED run recorded (tests written with code).
- Task 2: minor (deferred): some i3 default-config comments paraphrased, not verbatim (tiling_drag, launcher).
- Task 2: minor (deferred): default_config header says "re-run i3win to configure it again" — inaccurate wording.
- Task 3: minor (deferred): arrange_workspace sets ctx.g after the fullscreen early-return; floating placement on a fullscreen ws sees the previous ws's inner gap (harmless today).
- Task 3: minor (deferred): no tests for vertical/right/bottom, per-workspace inner affecting window gap, or workspace gaps/output disambiguation.
- Task 4: minor (deferred): deco.rs ICON_CACHE/ICON_REQUESTED never evicted; recycled HWND keeps a stale icon.
- Task 4: minor (deferred): deco.rs fills the whole frame; comment about per-side widths misleading.
- Task 4: minor (deferred): no tests for hide_edge_borders vertical/horizontal alone, smart_no_gaps, smart_borders no_gaps, smart_gaps inverse_outer.
- Task 4: minor (deferred): border command lacks negative-width validation; title_window_icon accepts extra aliases; multi-line string literals in tests.
- Task 5: minor (deferred): thin tests (multi-side toggle, all clearing non-inner override, px suffix/missing amount, clamp for all); no parse_gaps unit tests; trailing tokens ignored.
- Task 6: minor (deferred): thin tests for __focused__ instance/title/workspace/con_id, id/con_mark negative paths, urgent combined with other keys.
- Task 7: minor (deferred): `[criteria] unmark` (no name) clears all marks instead of only matched (i3 scopes to matched).
- Task 7: minor (deferred): `is_leaf` closure misnamed (marks.rs); no tests for floating swap success, move to floating mark, mark-removal event on previous holder, swap with con_id/id success.
- Task 7: minor (deferred): fallback branch (focused window moved to hidden ws) leaves focused_container uncleared.
- Task 8: minor (deferred): send_to_scratch runs a full arrange per window moved.
- Task 9: minor (deferred): no tests for focused sticky staying focused, multiple sticky windows, check_invariants after switch, sticky JSON/parse.
- Task 10: minor (deferred): no test for `workspace number N` auto back-and-forth or rename re-sort position; redundant __ check (parser + rename).
- Task 11: minor (deferred): plan's force test doesn't discriminate Force from Yes (needs nested same-orientation case); no tests for Workspace skipping outputs, No/Force with 2 outputs, floating focus next.
- Task 12: minor (deferred): no_focus window on an unfocused workspace (not alone) still becomes that ws's focus-within target via set_focused_within.
- Task 12: minor (deferred): activation Focus branch doesn't emit window "focus" IPC event.
- Task 12: minor (deferred): tests don't assert urgent cleared on activation focus; Smart-different-class/Urgent paths untested.
- Task 13: minor (deferred): `resize set height 40 50` silently overwrites; tiled px converted vs output size (consistent with v1 resize); no tests for sibling rescale, height axis, clamp in move_direction/on_floating_moved.
- Task 14: minor (deferred): no tests for move window to output, __i3 rejected as target, layout toggle staying-in-split path, bad list token.
- Task 15: minor (deferred): bindcode accepts modifier keycodes (133, 105, 37...) that can never fire — silent dead binding, could warn.
- Task 15: minor (deferred): auto-repeat key-down with different mods drops a pending release binding (pre-existing pattern).
- Task 16: minor (deferred): live script gaps reset assumes 0; "9" regex loose; no test for urgent-cleared event; floating disable after scratchpad show assumes focus.

### plan6-bar-markup

- Task 1: minor (deferred, carry to T5): apply_size accepts nan/inf/negative scale/size_pt — T5 must clamp font sizes to a sane finite range when creating fonts.
- Task 1: minor (deferred): &#0; and &#+65; accepted; attribute values not entity-decoded; no explicit hostile-input no-panic table test.
- Task 2: minor (deferred): click_writer_framing test weakened to starts_with; MinWidth::Px rejects floats (whole array dropped).
- Task 3: minor (deferred): no tests for mode parsing, Hide/Invisible reserving (0,0), mode in GET_BAR_CONFIG. Temporary: a first bar in hide mode still drawn until T6/T7.
- Task 4: minor (deferred): GET_BAR_CONFIG modifier lossy for combined modifiers; redundant borders/markup assignment; focused_separator/show_buttons untested.
- Task 5: minor (deferred): font cache unbounded across distinct markup font specs (GDI handle pressure); separator measure_text GetDC per paint; border fills not clamped to rect.
- Task 6: minor (deferred): duplicate explicit bar ids collide in statuses/windows maps (dedupe at parse); all_blocks deep-clones per refresh; route_click test-only helper.
- Task 7: minor (deferred): App.modifier_held not re-synced when the watch changes (bar may not reveal if modifier already held); different-modifier hide bars revealed by the first bar's modifier; hide overlays at edge offset 0; visibility rule untested; test placement in keyboard.rs.
- Task 8: minor (deferred): wheel→button mapping inline/untested; release bindings fire on any button-up (no capture); press still runs default when only --release bound.

### plan7-status

- Task 1: minor (deferred): parse_value accepts nan/inf as numbers; missing } mid-file reports wrong error; module-level allow(dead_code) to remove later; no non-ASCII/CRLF test.
- Task 2: minor (deferred): no test documenting that inline '#' in values stays verbatim; test paths Windows-only.
- Task 3: minor (deferred): redundant Num arm in min_width; format_bytes 1023.97 KiB rounds to 1024.0 KiB; markup value unvalidated.
- Task 4: minor (deferred): SYSTEM_POWER_STATUS→BatteryState mapping untested; %emptytime uses Local::now (untestable); trailing space when %remaining empty; disk threshold variants untested.
- Task 5: minor (deferred): TransmitLinkSpeed u64::MAX printed as huge Mbit/s; wifi up-but-query-fails shows "W: ( at ) ip" Good (should use format_down); ptr::eq index lookup; read_wifi on down adapter.
- Task 6: minor (deferred): $1/$?/$@ left literal; no pathological-expansion no-panic test.
- Task 7: minor (deferred): cmd missing command exits 1 (documented, no code change); modules render on event-loop thread; repeat block respawn tight loop (i3blocks parity); job assignment race (v1 parity).
- Task 7: minor (deferred): output printed >150 ms after process exit dropped (accepted trade-off); Quit-on-stdin-EOF also applies in plain output mode.
- Task 8: minor (deferred): `--signal abc`/bare `--signal` starts a full status process instead of erroring; RegisterClassW result ignored.

### plan8-mouse

- Task 1: minor (deferred): resize_from_corner i32 cast can truncate; "skips tabbed" untested; node_rect runs full arrange per call (watch drag perf).
- Task 2: minor (deferred): drop_window doesn't reject a fullscreen dragged window.
- Task 3: minor (deferred): >64 queued button events reallocate in hook; floating focus ranks compared across workspaces; on_move &mut self.
- Task 3: minor (deferred): menu-mask dummy key sent from main thread — a very fast modifier release can still race (same as keyboard Mask path).
- Task 4: minor (deferred): stuck ESC_SWALLOWED after missed Escape key-up; swallowed press without a drag still focuses/flushes.
- Task 4: minor (deferred): drag timer not killed on end/cancel (harmless); full queue loses the coalesced move too; a lost hook Release leaves the drag until Escape (accepted).
- Task 5: minor (deferred): Resized(vec![]) when width change split across sub-slop edges; Up/positive Down untested.
- Task 6: minor (deferred): 10 s freeze expiry thaws a genuinely long native drag; hover_at/classify_point glue untested; wheel delta sign unverified live.
- Task 7: minor (deferred): unmatched button-ups in finally; force-kill leaves ws1 notepads hidden (killed after); weak swap assertion; hover check can pick a stranger window; no-config branch has no bindings.
