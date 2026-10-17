# Build rulings (decisions made during execution)

Every judgment call the controller made while running Plans 1-4, in order, with what it costs if wrong. Parked items are known gaps left for the backlog.

- Ruling: work on branch feat/i3win in the main checkout instead of a separate worktree — repo has only docs, nobody else works in it — cost if wrong: none (branch is isolated from main).
- Ruling: commit Cargo.lock (Task 1 added it unlisted) — workspace ships binaries; lockfile gives reproducible builds — cost if wrong: trivial removal.
- Ruling: keep "Co-Authored-By: Claude Opus 5.5" trailer from constraints even though implementers run on Sonnet — controller's attribution instruction specifies it; consistency across commits — cost if wrong: cosmetic trailer edit.
- Ruling: added "run cargo fmt before committing" to constraints for later tasks — fixes Task 1 formatting minor going forward — cost if wrong: none.
- Ruling: Task 4 swap no-op on ancestor/unparented/floating nodes and wrap of unparented node — sensible defaults the plan left open — cost if wrong: small later fix.
- Ruling: Wm::new with zero outputs uses a synthetic 1920x1080 output 'FALLBACK' instead of panicking — spec forbids panics on OS data — cost if wrong: minor.
- Ruling: Task 7 plan-mandated defect — Wm::new fallback must skip names reserved by workspace_outputs for OTHER outputs (spec: "workspace NAME output OUT" must be honored) — cost if wrong: none, strictly better.
- Ruling: non-window commands (Exec, Reload, Restart, Exit, Mode, Nop) run once per command even when criteria match several windows — matches i3 — cost if wrong: trivial.
- Ruling: Plan defect — focus-path model can't represent a focused container (focus parent). Accept Task 9's Wm.focused_container: Option<(container, leaf)>, valid only while the path leaf equals the stored leaf AND container still exists AND is an ancestor of the leaf — spec requires i3 focus parent semantics — cost if wrong: focus-parent edge bugs, contained to Wm::focused().
- Ruling: fix C1, I1-I8 and minors 1,2,5,7 in one fix wave; minors 3 (window lookup index), 4 (manage event order), 6 (fallback name) deferred. All are small and Plan 2 builds on them. Cost if wrong: a larger fix diff.
- Ruling: I6 keep fullscreen but still place the workspace's floating windows above it so dialogs stay visible. Matches i3 and user expectation. Cost if wrong: dialogs over fullscreen apps.
- Ruling: I2 supersedes the Carry-to-Plan-2-Task-7 note. Core emits Window focus with every Effect::Focus via push_focus; on_focus_changed is a no-op when the node is already the focus-path leaf. Plan 2 must NOT filter Foreground events itself. Cost if wrong: duplicate focus IPC events.
- Ruling: Windows-path backslash-escape (Task 5 minor) deferred per final reviewer: fails cleanly with unterminated quote, no panic; fix before release by accepting \ escapes. Cost if wrong: users must avoid trailing backslash in quoted paths.
- Plan1: parked - I2/I3 regression tests vacuous (focus parent on direct workspace children is a no-op, so no selection is made) - Ruling: code verified correct by re-review; tests need tree.wrap to build a real container; deferred to backlog, no second fix wave allowed - cost if wrong: focus-selection regressions go uncaught by tests.
- Plan1: parked - manage emits Window focus before new (i3 order is new then focus) - Ruling: cosmetic IPC ordering, fix in Plan 4 when IPC events are serialized - cost if wrong: strict IPC clients see odd order.
- Plan1: parked - deferred minors 3 (window_node linear scan), 6 (fallback output name 'default' vs ledger FALLBACK) - Ruling: perf/cosmetic - cost if wrong: minor.
- Ruling: Task 7 flush order — apply Exec/Close/Exit/Reload effects first, then arrangement ops (Hide, Show, Place), then Focus effects last — focusing a still-hidden window fails — cost if wrong: none.
- Ruling: fullscreen covers the work area (rcWork) only, taskbar stays visible in v1 — spec non-goal "not hiding the taskbar" — cost if wrong: users expecting true fullscreen; fix later by adding a full monitor rect to OutputInfo.
- Ruling: controller committed .gitattributes (eol=lf) as 17841ab — implementers kept producing CRLF/mixed endings — cost if wrong: trivial.
- Ruling: P2 Task 2 — ShowWindow on foreign windows can block on a hung app, violating the spec anti-goal 'freezing because one app hangs'; switch hide/show/restore to ShowWindowAsync (fix round 1) — cost if wrong: none.
- Ruling: P2 Task 4 fix round 1 re-review done by controller inspection (one-literal fix, verified line 14 and tests 84-85) instead of a reviewer dispatch — fix is trivially checkable — cost if wrong: final review catches it.
- Ruling: subagents must NOT run i3win.exe live (Task 7/8 manual checks, Task 9 smoke test) — it would tile/hide the user's real open windows on their desktop while they may be using it; those checks become a hand-off checklist for the user — cost if wrong: platform bugs found later by the user instead of now.
- Ruling: P2 Task 7 fix round 1 re-review done by controller inspection of the 2-hunk diff (set_outputs call; POISONED flag) — trivially checkable — cost if wrong: final review catches it.
- Ruling: commit 87d7b9f carries a 'Claude Sonnet 5.5' trailer instead of Opus — left as-is rather than rewriting history — cost if wrong: cosmetic attribution inconsistency.
- Ruling: fix C1, I1-I9(log), triage T1/T2 and minors M1/M3/M7 in one fix wave; also commit docs/manual-checks.md — all small/local and Plan 3 builds on them — cost if wrong: larger diff.
- Ruling: GetWindowTextW on foreign windows accepted as non-blocking (reads stored caption, per final reviewer citing MSDN) — cost if wrong: hang on a hung window title read.
- Plan2: parked - park_foreground targets the invisible main window; SetForegroundWindow on an invisible window may fail so typing on an empty workspace could still reach a hidden app - Ruling: verify via manual check 3 + debug log; Plan 3 bar windows are a visible fallback target - cost if wrong: keystrokes into hidden app on empty workspace.
- Plan2: parked - restore_all_hidden mirror lock could deadlock if main thread panics while holding it - Ruling: requires OOM inside the lock-held clone; deferred - cost if wrong: hang instead of restore in an OOM panic.
- Ruling: StatusUpdate (1 Hz) refreshes only the bars, not a full flush — full flush re-orders every deco window each second — cost if wrong: none.
- Ruling: fix I1, I2, wheel accumulator, M1 backslash escapes (also resolves Plan 1 parked Windows-path issue), M2 cmd /S /C quoting for status_command and exec, M3 64KiB line cap, M4 docs in one fix wave — cheap and Plan 4 IPC builds on post() — cost if wrong: larger diff.
- Ruling: quoted criteria values now unescape \ to \ (consistent with commands); regex escapes like \. or \d are unchanged, only a literal-backslash regex needs four backslashes — cost if wrong: rare config incompatibility.
- Plan3: parked - quoted UNC paths: exec "\server\share" now collapses to one leading backslash under the new \ escape - Ruling: document; users write four backslashes or leave the path unquoted - cost if wrong: confusing UNC path configs.
- Plan3: parked - create_main_window's initial PostMessageW failure would leave WAKE_PENDING set - Ruling: essentially impossible; deferred - cost if wrong: main msgs never processed.
- Ruling: Task 2 also fixes the Plan 1 parked event order: manage emits Window "new" before Window "focus" — i3 parity, IPC is where it's observable — cost if wrong: none.
- Ruling: P4 Task 2 test run_command_reply changed from 'nop; frob' to 'nop; mode nosuch' — parse errors reject the whole command string (Plan 1 behavior, matches i3), so the plan's test was wrong — cost if wrong: none.
- Ruling: controller committed the Cargo.lock line Task 3 left uncommitted (serde_json already in crates/i3win/Cargo.toml) — trivial bookkeeping — cost if wrong: none.
- Ruling: fix I1, I2 plus cheap minors M1 bounded channel, M4 PIPE_BUSY retry, M6 global I3SOCK, M9 shared test lock, M10 failed-subscribe exit, stdout BrokenPipe, test literals; append IPC manual checks — cost if wrong: larger final diff.
- Ruling: controller corrected the docs/manual-checks.md item 9 pipe paths (missing backslash) and documented the smoke-test caveat — one-line doc fix, no second fix wave allowed — cost if wrong: none.
- Plan4: parked - I2 residual: ipc_roundtrip_and_events kills i3win on assertion failure without sending exit (windows on other workspaces stay hidden until next i3win start restores them from hidden.txt); its success-path exit uses read_frame().unwrap() and could panic if i3win exits before replying; the wait-then-kill wrapper landed on the wrong test - Ruling: backlog; mitigated by Plan 2 startup recovery and documented in manual check 12 - cost if wrong: user's windows hidden until they start i3win again.

## Live verification (2026-10-17)

- Ruling: the keyboard hook now ignores only injected keys tagged with OWN_INPUT_TAG (i3win's own dummy key); other injected input fires bindings — it previously ignored all injected input, which broke AutoHotkey remaps, on-screen keyboards, remote tools and automated testing — cost if wrong: a tool that injects Alt+key combos could trigger bindings.
- Live results: both smoke tests pass (the tiling one needs a desktop with no other restored windows); scripts/live-checks.ps1 passes 45/45; panic recovery (I3WIN_PANIC_AFTER_SECS) restores hidden windows and logs the panic.
- Parked item resolved: park_foreground — typing on an empty workspace did not reach a hidden Notepad in the live run.
