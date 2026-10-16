# i3win Plan 2 — Win32 Platform Layer and Decorations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `i3win.exe`, which tiles real Windows 10 windows using `i3win-core`: global keybindings, workspace hiding, focus, i3-style title bars, borders and tab bars, and it never strands hidden windows.

**Architecture:** Single-threaded UI. The main thread owns a hidden top-level window (`i3win_main`), a WinEvent hook, a low-level keyboard hook and the decoration windows. Hook callbacks only push into thread-local queues and `PostMessage` the main window. The main window's wndproc drains the queues into `App`. `App` feeds `Wm`, then "flushes": applies `Effect`s, computes `Wm::arrange()`, and reconciles real windows and decoration windows against what was last applied. Win32 decisions are pure functions or structs wherever possible so they can be unit-tested.

**Tech Stack:** Rust 2021, `windows = "0.58"`, `log = "0.4"`, `simplelog = "0.12"`, `i3win-core` (Plan 1).

**Spec:** `docs/superpowers/specs/2026-10-16-i3win-design.md`
**Depends on:** Plan 1 completed (`docs/superpowers/plans/2026-10-16-plan1-core.md`). Read the core's public API (`lib.rs`, `wm.rs`, `arrange.rs`) before starting any task.

## Global Constraints

- Everything from Plan 1's Global Constraints applies (repo, toolchain, commit trailer, no panics on OS data, `cargo clippy -- -D warnings`).
- `windows` crate pinned to `0.58`. In 0.58, `HWND` is `HWND(pub *mut c_void)` and is not `Send`. Convert with `HWND(id.0 as *mut _)` / `WindowId(hwnd.0 as isize)`. Only `WindowId`/`isize` crosses threads.
- Hook callbacks (`SetWinEventHook`, `WH_KEYBOARD_LL`) must not touch `App` or `Wm`. They push into a thread-local queue and `PostMessageW` the main window. The keyboard hook must do no allocation-heavy work, no I/O and no logging on the hot path.
- `App` lives in a `thread_local! { static APP: RefCell<Option<App>> }`. Wndprocs use `try_borrow_mut`; if it is already borrowed, re-post the same message instead of panicking.
- Never call `SendMessage` on a foreign window. Use `PostMessageW` (`WM_CLOSE`), `SetWindowPos` with `SWP_ASYNCWINDOWPOS`, and `ShowWindowAsync`-equivalent flags where noted.
- Window classes we register start with `i3win_`. Our own process's windows are never managed.
- `i3win.exe` uses `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`.
- File paths: config `%USERPROFILE%\.config\i3win\config` (override: `--config <path>` CLI argument); log `%LOCALAPPDATA%\i3win\i3win.log`; hidden-window store `%LOCALAPPDATA%\i3win\hidden.txt`.
- Run `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` before each commit.

## Review Focus

1. **The keyboard hook gets silently removed** if a callback takes longer than ~300 ms. Any blocking in the hook path breaks every binding until restart. → test `matcher_is_pure_and_fast` in Task 6 (10k events under 50 ms) plus code review: no locks, I/O or `log!` in `ll_proc`.
2. **A crash or kill while windows are hidden on other workspaces.** On the next start every HWND in `hidden.txt` that still exists must be shown again. → test `store_roundtrip_and_take_previous` in Task 3 and manual check in Task 9.
3. **Our own hide events read as "window closed".** Without the expected-hide tracker, switching workspaces would unmanage every window. → test `tracker_consumes_expected_hides_and_expires` in Task 3.
4. **Windows that show before they have a title** (common with Chromium and Electron apps) must still get managed once the title appears. → test `title_change_on_unmanaged_retries_manage` in Task 7.
5. **The modifier key stuck down after Win+L or UAC** (the key-up goes to the secure desktop). Bindings must recover. → test `resync_clears_stuck_modifiers` in Task 6.

---

## File Structure

```
crates/i3win/
  Cargo.toml
  src/main.rs              startup/shutdown sequence, CLI args
  src/paths.rs             config/log/hidden paths, default-config writing, wt detection
  src/mainloop.rs          main window class, message loop, cross-thread MainMsg queue, private WM_APP ids
  src/app.rs               App: Wm + platform state, event dispatch, flush
  src/reconcile.rs         pure diff: Arrangement vs applied state → WinOp list
  src/win/mod.rs
  src/win/window.rs        HWND helpers: info, classification, frame compensation, place/show/hide/close/focus
  src/win/hidden.rs        HideTracker (expected hides) + HiddenStore (hidden.txt) + restore_all
  src/win/monitors.rs      enumerate outputs
  src/win/events.rs        WinEvent hook → PlatformEvent queue
  src/win/keyboard.rs      KeyMatcher (pure) + WH_KEYBOARD_LL hook
  src/win/deco.rs          decoration windows: reconcile + GDI painting
  src/win/gdi.rs           font cache, color helpers, double-buffered paint helper
  tests/smoke.rs           #[ignore] end-to-end test with Notepad
```

---

### Task 1: Binary scaffold, paths, logging, single instance, main loop

**Files:**
- Create: `crates/i3win/Cargo.toml`, `src/main.rs`, `src/paths.rs`, `src/mainloop.rs`, `src/win/mod.rs`
- Modify: root `Cargo.toml` (members += `"crates/i3win"`)

**Interfaces:**
- Produces:
  - `paths::config_path(cli_override: Option<PathBuf>) -> PathBuf`, `paths::data_dir() -> PathBuf` (`%LOCALAPPDATA%\i3win`, created if missing), `paths::log_path() -> PathBuf`, `paths::hidden_path() -> PathBuf`
  - `paths::find_on_path(exe: &str, path_var: &str) -> bool` (pure: splits `path_var` on `;`, checks `dir\exe` exists)
  - `paths::load_or_create_config(path: &Path) -> Result<String, String>`: if the file is missing, create parent dirs and write `i3win_core::config::default_config_text(find_on_path("wt.exe", &env PATH))`, then return the text.
  - `mainloop::WM_APP_EVENTS = WM_APP + 1`, `WM_APP_KEY = WM_APP + 2`, `WM_APP_MAIN = WM_APP + 3`
  - `pub enum MainMsg { FocusNode(i3win_core::tree::NodeId), Shutdown }` (Plans 3–4 add variants)
  - `mainloop::post(msg: MainMsg)` — callable from any thread: pushes into a `static Mutex<VecDeque<MainMsg>>` and `PostMessageW(main_hwnd, WM_APP_MAIN)`. The main HWND is stored in a `static MAIN_HWND: AtomicIsize`.
  - `mainloop::create_main_window() -> windows::core::Result<HWND>` — registers class `i3win_main`, creates an invisible `WS_POPUP` with `WS_EX_TOOLWINDOW` (a real top-level window, so it receives `WM_DISPLAYCHANGE`/`WM_SETTINGCHANGE`/`WM_ENDSESSION` broadcasts).
  - `mainloop::run() -> i32` — `GetMessageW` loop until `WM_QUIT`; returns its exit code.
  - `mainloop::drain_main_msgs() -> Vec<MainMsg>`

`Cargo.toml` for `i3win`:
```toml
[package] name = "i3win"  version = "0.1.0"  edition = "2021"
[dependencies]
i3win-core = { path = "../core" }
log = "0.4"
simplelog = "0.12"
[dependencies.windows]
version = "0.58"
features = ["Win32_Foundation","Win32_UI_WindowsAndMessaging","Win32_UI_Accessibility","Win32_UI_Input_KeyboardAndMouse","Win32_UI_HiDpi","Win32_Graphics_Gdi","Win32_Graphics_Dwm","Win32_System_Threading","Win32_System_LibraryLoader","Win32_System_Console","Win32_Security","Win32_System_Pipes","Win32_Storage_FileSystem","Win32_System_IO","Win32_System_SystemServices"]
```

`main.rs` startup in this task (later tasks add steps at the marked points):
1. `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)` (ignore error).
2. Parse args: `--config <path>`; `--version` prints `i3win 0.1.0` and exits 0.
3. Init `simplelog::WriteLogger` to `log_path()` (append), level `Info`, or `Debug` if env `I3WIN_LOG=debug`. In debug builds also add a `TermLogger`.
4. Single instance: `CreateMutexW(None, true, w!("Local\\i3win-single-instance"))`; if `GetLastError() == ERROR_ALREADY_EXISTS` → log "already running" and exit 1.
5. `create_main_window()`; `run()`; exit with its code.

The main wndproc handles `WM_APP_MAIN` by draining `MainMsg`s; `Shutdown` → `PostQuitMessage(0)`. `WM_CLOSE` → `PostQuitMessage(0)`.

- [ ] **Step 1: Write failing tests in `paths.rs`**

```rust
#[test] fn find_on_path_checks_each_dir() {
    let dir = std::env::temp_dir().join("i3win_test_path"); std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fake.exe"), b"").unwrap();
    let pv = format!("C:\\nonexistent;{}", dir.display());
    assert!(find_on_path("fake.exe", &pv));
    assert!(!find_on_path("other.exe", &pv));
}
#[test] fn creates_default_config_when_missing() {
    let p = std::env::temp_dir().join("i3win_test_cfg").join("config");
    let _ = std::fs::remove_file(&p);
    let text = load_or_create_config(&p).unwrap();
    assert!(p.exists() && text.contains("bindsym $mod+Return"));
    assert_eq!(load_or_create_config(&p).unwrap(), text);
}
```

- [ ] **Step 2: Run** `cargo test -p i3win` — Expected: FAIL.
- [ ] **Step 3: Implement** `paths.rs`, `mainloop.rs`, `main.rs`, `win/mod.rs` as specified.
- [ ] **Step 4: Run** `cargo test -p i3win` — Expected: PASS. Then `cargo run -p i3win -- --version` prints `i3win 0.1.0`. Then start `cargo run -p i3win` in the background, check that a second `cargo run -p i3win` exits with code 1 and that the log contains "already running", and stop the first instance with `taskkill /IM i3win.exe /F`.
- [ ] **Step 5: Commit** `feat(platform): binary scaffold, paths, single instance, main loop`

---

### Task 2: Window helpers — info, classification, frame compensation, control

**Files:**
- Create: `crates/i3win/src/win/window.rs`; Modify: `win/mod.rs`

**Interfaces:**
- Consumes: `i3win_core::{geom::Rect, tree::{WindowId, WindowInfo}}`.
- Produces:
  - `pub struct WindowProps { pub visible: bool, pub style: u32, pub ex_style: u32, pub has_owner: bool, pub cloaked: bool, pub own_process: bool, pub title: String, pub class_name: String, pub is_child: bool }`
  - `#[derive(Debug, PartialEq, Eq)] pub enum Manage { No, Tile, Float }`
  - `pub fn classify(p: &WindowProps) -> Manage` (pure)
  - `pub fn compensate(target: Rect, window_rect: Rect, frame_rect: Rect) -> Rect` (pure): returns the rect to pass to `SetWindowPos` so the visible frame equals `target`: `x = target.x - (frame.x - win.x)`, `y = target.y - (frame.y - win.y)`, `w = target.w + (frame.x - win.x) + (win.right() - frame.right())`, `h = target.h + (frame.y - win.y) + (win.bottom() - frame.bottom())`.
  - `pub fn exe_stem(path: &str) -> String` (pure): `C:\Program Files\Mozilla Firefox\firefox.exe` → `firefox` (file name without extension, case kept).
  - Win32 wrappers: `pub fn props(id: WindowId) -> Option<WindowProps>`, `pub fn info(id: WindowId, wants_float: bool) -> Option<WindowInfo>` (title via `GetWindowTextW`, class via `GetClassNameW`, exe via `GetWindowThreadProcessId` + `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW` → `exe_stem`, rect = DWM extended frame bounds), `pub fn frame_rect(id) -> Option<Rect>` (`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`, falling back to `GetWindowRect`), `pub fn place(id, target: Rect, raise: bool)` (SW_RESTORE if `IsZoomed`; compute `compensate` from `GetWindowRect` + `frame_rect`; `SetWindowPos(hwnd, if raise {HWND_TOP} else {None}, ..., SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_NOOWNERZORDER | (if !raise {SWP_NOZORDER}))`; failures → `log::warn!` once per window), `pub fn hide(id) -> bool` (returns whether it was visible: `IsWindowVisible` then `ShowWindow(SW_HIDE)`), `pub fn show(id)` (`ShowWindow(SW_SHOWNA)`), `pub fn close(id)` (`PostMessageW(WM_CLOSE)`), `pub fn focus(id)` (if `IsIconic` → SW_RESTORE; `send_dummy_key()`; `SetForegroundWindow`), `pub fn send_dummy_key()` (`SendInput` of VK `0xE8` down then up), `pub fn is_window(id) -> bool`, `pub fn enumerate_top_level() -> Vec<WindowId>` (`EnumWindows`, top of the z-order first), `pub fn foreground() -> Option<WindowId>`.

`classify` rules (in order):
1. `!visible || is_child || has_owner || cloaked || own_process || title.is_empty()` → `No`.
2. `ex_style & WS_EX_TOOLWINDOW (0x80) != 0` → `No`. `ex_style & WS_EX_NOACTIVATE (0x0800_0000) != 0` → `No`.
3. `style & WS_CAPTION (0x00C0_0000) != WS_CAPTION` → `No`.
4. `class_name` in `["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Progman", "WorkerW", "Windows.UI.Core.CoreWindow", "TaskManagerWindow"]` → `No`.
5. `ex_style & WS_EX_DLGMODALFRAME (0x1) != 0` or `style & WS_THICKFRAME (0x0004_0000) == 0` → `Float`.
6. otherwise `Tile`.

Fill `WindowProps` with: `visible` = `IsWindowVisible`; `style`/`ex_style` = `GetWindowLongW(GWL_STYLE / GWL_EXSTYLE)`; `has_owner` = `GetWindow(GW_OWNER)` non-null; `cloaked` = `DwmGetWindowAttribute(DWMWA_CLOAKED)` ≠ 0; `own_process` = pid == `GetCurrentProcessId()`; `is_child` = `style & WS_CHILD`.

- [ ] **Step 1: Write failing tests (pure functions)**

```rust
fn base() -> WindowProps { WindowProps { visible: true, style: 0x00C0_0000 | 0x0004_0000, ex_style: 0, has_owner: false, cloaked: false, own_process: false, title: "t".into(), class_name: "Notepad".into(), is_child: false } }
#[test] fn normal_window_tiles() { assert_eq!(classify(&base()), Manage::Tile); }
#[test] fn rejects() {
    let fs: [fn(&mut WindowProps); 8] = [|p| p.visible = false, |p| p.has_owner = true, |p| p.cloaked = true, |p| p.own_process = true, |p| p.title.clear(), |p| p.ex_style = 0x80, |p| p.style = 0x0004_0000, |p| p.class_name = "Shell_TrayWnd".into()];
    for f in fs {
        let mut p = base(); f(&mut p); assert_eq!(classify(&p), Manage::No);
    }
}
#[test] fn dialogs_and_fixed_size_float() {
    let mut p = base(); p.ex_style = 0x1; assert_eq!(classify(&p), Manage::Float);
    let mut p = base(); p.style = 0x00C0_0000; assert_eq!(classify(&p), Manage::Float);
}
#[test] fn compensate_for_invisible_borders() {
    // window rect is 7px wider on left/right/bottom than its visible frame
    let win = Rect::new(93, 100, 414, 307); let frame = Rect::new(100, 100, 400, 300);
    assert_eq!(compensate(Rect::new(0, 0, 500, 500), win, frame), Rect::new(-7, 0, 514, 507));
}
#[test] fn exe_stem_strips_dir_and_ext() { assert_eq!(exe_stem(r"C:\Program Files\Mozilla Firefox\firefox.exe"), "firefox"); assert_eq!(exe_stem("Code.exe"), "Code"); }
```

- [ ] **Step 2: Run** `cargo test -p i3win` — Expected: FAIL.
- [ ] **Step 3: Implement `window.rs`.**
- [ ] **Step 4: Run tests** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(platform): window queries, classification and control`

---

### Task 3: Expected-hide tracker, hidden-window store, restore-all

**Files:**
- Create: `crates/i3win/src/win/hidden.rs`; Modify: `win/mod.rs`

**Interfaces:**
- Produces:
  - `pub struct HideTracker { pending: HashMap<WindowId, Vec<Instant>> }` with `new()`, `expect(&mut self, id, now: Instant)`, `consume(&mut self, id, now: Instant) -> bool` (drops entries older than 2 s first; true if one was removed), `forget(&mut self, id)`.
  - `pub struct HiddenStore { path: PathBuf, set: BTreeSet<isize> }` with `open(path) -> HiddenStore` (starts empty), `add(&mut self, id: WindowId)`, `remove(&mut self, id: WindowId)`, `contains(&self, id) -> bool`, `ids(&self) -> Vec<WindowId>`, `clear_file(&mut self)`. Every mutation rewrites the file (one decimal id per line) and updates a process-global `static HIDDEN_MIRROR: Mutex<BTreeSet<isize>>`.
  - `pub fn take_previous(path: &Path) -> Vec<WindowId>` — parses the file (skipping bad lines), deletes it, returns the ids.
  - `pub fn restore_all_hidden()` — for every id in `HIDDEN_MIRROR`: if `window::is_window` → `window::show`. Clears the mirror. Must be safe to call from a panic hook or console-control handler (no `RefCell`, no `App`).

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn tracker_consumes_expected_hides_and_expires() {
    let t0 = Instant::now(); let mut t = HideTracker::new(); let w = WindowId(5);
    t.expect(w, t0);
    assert!(t.consume(w, t0 + Duration::from_millis(10)));
    assert!(!t.consume(w, t0 + Duration::from_millis(20)));      // only one expected
    t.expect(w, t0);
    assert!(!t.consume(w, t0 + Duration::from_secs(3)));          // expired
}
#[test] fn store_roundtrip_and_take_previous() {
    let p = std::env::temp_dir().join("i3win_test_hidden.txt"); let _ = std::fs::remove_file(&p);
    let mut s = HiddenStore::open(p.clone());
    s.add(WindowId(10)); s.add(WindowId(20)); s.remove(WindowId(10));
    std::fs::write(&p, format!("{}\ngarbage\n30\n", std::fs::read_to_string(&p).unwrap().trim())).unwrap();
    assert_eq!(take_previous(&p), vec![WindowId(20), WindowId(30)]);
    assert!(!p.exists());
}
```

- [ ] **Step 2: Run tests** — Expected: FAIL.
- [ ] **Step 3: Implement `hidden.rs`.**
- [ ] **Step 4: Run tests** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(platform): hide tracking and hidden-window persistence`

---

### Task 4: Monitors

**Files:**
- Create: `crates/i3win/src/win/monitors.rs`; Modify: `win/mod.rs`

**Interfaces:**
- Produces: `pub fn output_name(device: &str) -> String` (pure: strips a leading `\\.\`; `\\.\DISPLAY1` → `DISPLAY1`), `pub fn sort_outputs(v: &mut Vec<OutputInfo>)` (pure: by rect.x, then rect.y), `pub fn enumerate() -> Vec<OutputInfo>` (`EnumDisplayMonitors` + `GetMonitorInfoW` with `MONITORINFOEXW`: name from `szDevice`, rect = `rcWork`; scale = `GetDpiForMonitor(MDT_EFFECTIVE_DPI).x / 96.0`, default 1.0 on error; sorted).

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn names() { assert_eq!(output_name(r"\\.\DISPLAY2"), "DISPLAY2"); assert_eq!(output_name("X"), "X"); }
#[test] fn sorted_left_to_right() {
    let mut v = vec![OutputInfo { name: "B".into(), rect: Rect::new(1920, 0, 1920, 1040), scale: 1.0 }, OutputInfo { name: "A".into(), rect: Rect::new(0, 0, 1920, 1040), scale: 1.0 }];
    sort_outputs(&mut v); assert_eq!(v[0].name, "A");
}
#[test] fn enumerate_finds_at_least_one() { assert!(!enumerate().is_empty()); }
```

- [ ] **Step 2: Run tests** — Expected: FAIL.
- [ ] **Step 3: Implement.** Add `"Win32_UI_HiDpi"` usage for `GetDpiForMonitor` (it's in `Win32::UI::HiDpi`).
- [ ] **Step 4: Run tests** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(platform): monitor enumeration`

---

### Task 5: WinEvent hook

**Files:**
- Create: `crates/i3win/src/win/events.rs`; Modify: `win/mod.rs`

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum PlatformEvent { Shown(WindowId), Hidden(WindowId), Destroyed(WindowId), Foreground(WindowId), Minimized(WindowId), Restored(WindowId), MoveSizeEnd(WindowId), TitleChanged(WindowId) }`
  - `pub fn translate(event: u32, hwnd: isize, id_object: i32, id_child: i32) -> Option<PlatformEvent>` (pure)
  - `pub fn install() -> windows::core::Result<()>` — two `SetWinEventHook(min, max, None, Some(callback), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)` calls: system range `0x0003..=0x0017` and object range `0x8001..=0x8018`. Stores the hook handles for `uninstall()`.
  - `pub fn uninstall()`, `pub fn drain() -> Vec<PlatformEvent>`.
  - The callback: `translate`; if `Some`, push into `thread_local! QUEUE: RefCell<Vec<PlatformEvent>>`; if the queue was empty before the push, `PostMessageW(main, WM_APP_EVENTS)`.

`translate` mapping (only when `hwnd != 0 && id_object == 0 /*OBJID_WINDOW*/ && id_child == 0 /*CHILDID_SELF*/`, otherwise None):
`0x8002 EVENT_OBJECT_SHOW` and `0x8018 EVENT_OBJECT_UNCLOAKED` → Shown · `0x8003 EVENT_OBJECT_HIDE` and `0x8017 EVENT_OBJECT_CLOAKED` → Hidden · `0x8001 EVENT_OBJECT_DESTROY` → Destroyed · `0x0003 EVENT_SYSTEM_FOREGROUND` → Foreground · `0x0016 MINIMIZESTART` → Minimized · `0x0017 MINIMIZEEND` → Restored · `0x000B MOVESIZEEND` → MoveSizeEnd · `0x800C EVENT_OBJECT_NAMECHANGE` → TitleChanged · anything else → None.

- [ ] **Step 1: Write failing tests**

```rust
#[test] fn translates_known_events() {
    assert_eq!(translate(0x8002, 7, 0, 0), Some(PlatformEvent::Shown(WindowId(7))));
    assert_eq!(translate(0x8017, 7, 0, 0), Some(PlatformEvent::Hidden(WindowId(7))));
    assert_eq!(translate(0x0003, 7, 0, 0), Some(PlatformEvent::Foreground(WindowId(7))));
    assert_eq!(translate(0x800C, 7, 0, 0), Some(PlatformEvent::TitleChanged(WindowId(7))));
}
#[test] fn ignores_children_and_noise() {
    assert_eq!(translate(0x8002, 7, -4, 0), None);      // OBJID_CLIENT etc.
    assert_eq!(translate(0x8002, 7, 0, 3), None);
    assert_eq!(translate(0x800B, 7, 0, 0), None);       // LOCATIONCHANGE
    assert_eq!(translate(0x8002, 0, 0, 0), None);
}
```

- [ ] **Step 2: Run tests** — Expected: FAIL.
- [ ] **Step 3: Implement `events.rs`.**
- [ ] **Step 4: Run tests** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(platform): WinEvent hook`

---

### Task 6: Keyboard matcher and low-level hook

**Files:**
- Create: `crates/i3win/src/win/keyboard.rs`; Modify: `win/mod.rs`

**Interfaces:**
- Consumes: `i3win_core::keys::{KeyCombo, Modifiers}`.
- Produces:
  - `#[derive(Debug, PartialEq, Eq)] pub enum KeyAction { Pass, Swallow, Fire { combo: KeyCombo, mask_menu: bool } }`
  - `pub struct KeyMatcher { down_mods: HashSet<u16>, combos: HashSet<KeyCombo>, swallowed: HashSet<u16> }` with `new()`, `set_combos(&mut self, combos: &[KeyCombo])`, `on_key(&mut self, vk: u16, down: bool, injected: bool) -> KeyAction`, `mods(&self) -> Modifiers`, `resync(&mut self, is_down: impl Fn(u16) -> bool)`.
  - Hook: `pub fn install() -> windows::core::Result<()>`, `pub fn uninstall()`, `pub fn set_combos(combos: &[KeyCombo])` (writes the thread-local matcher), `pub fn drain() -> Vec<(KeyCombo, bool /*mask_menu*/)>`.

Matcher rules:
- Modifier VKs: Alt `0x12, 0xA4, 0xA5` → ALT; Shift `0x10, 0xA0, 0xA1` → SHIFT; Ctrl `0x11, 0xA2, 0xA3` → CTRL; Win `0x5B, 0x5C` → WIN.
- `injected == true` → `Pass` (our own `SendInput` dummy keys and other tools' injections).
- Modifier down → insert into `down_mods`, `Pass`. Modifier up → remove, `Pass`.
- Non-modifier down: `combo = KeyCombo { mods: self.mods(), vk }`; if in `combos` → insert vk into `swallowed`, return `Fire { combo, mask_menu: mods contains ALT or WIN }`; else `Pass`.
- Non-modifier up: if `swallowed.remove(vk)` → `Swallow`; else `Pass`.
- `resync(is_down)`: remove from `down_mods` every vk for which `is_down(vk)` is false.

Hook (`ll_proc`): on `HC_ACTION`, read `KBDLLHOOKSTRUCT` (`vkCode`, `flags`; injected = `flags & LLKHF_INJECTED (0x10)`), down = `wParam` is `WM_KEYDOWN` or `WM_SYSKEYDOWN`. For non-modifier key-downs, first call `resync(|vk| GetAsyncKeyState(vk as i32) < 0)` (i16 sign bit). `Fire` → push `(combo, mask_menu)` onto the thread-local queue, `PostMessageW(main, WM_APP_KEY)`, return `LRESULT(1)`. `Swallow` → return `LRESULT(1)`. `Pass` → `CallNextHookEx`. The main thread, on `WM_APP_KEY`: for each drained item, if `mask_menu` → `window::send_dummy_key()` first, then `app.on_binding(combo)`.

- [ ] **Step 1: Write failing tests**

```rust
use i3win_core::keys::parse_combo;
fn m(binds: &[&str]) -> KeyMatcher { let mut k = KeyMatcher::new(); k.set_combos(&binds.iter().map(|b| parse_combo(b).unwrap()).collect::<Vec<_>>()); k }
#[test] fn fires_and_swallows_up() {
    let mut k = m(&["Mod1+Return"]);
    assert_eq!(k.on_key(0xA4, true, false), KeyAction::Pass);
    assert_eq!(k.on_key(0x0D, true, false), KeyAction::Fire { combo: parse_combo("Mod1+Return").unwrap(), mask_menu: true });
    assert_eq!(k.on_key(0x0D, false, false), KeyAction::Swallow);
    assert_eq!(k.on_key(0xA4, false, false), KeyAction::Pass);
    assert_eq!(k.on_key(0x0D, true, false), KeyAction::Pass);         // no mod now
}
#[test] fn exact_modifier_match() { let mut k = m(&["Mod1+q"]); k.on_key(0xA4, true, false); k.on_key(0xA0, true, false); assert_eq!(k.on_key(0x51, true, false), KeyAction::Pass); }
#[test] fn bare_keys_dont_mask_menu() { let mut k = m(&["Escape"]); assert_eq!(k.on_key(0x1B, true, false), KeyAction::Fire { combo: parse_combo("Escape").unwrap(), mask_menu: false }); }
#[test] fn injected_passes() { let mut k = m(&["Escape"]); assert_eq!(k.on_key(0x1B, true, true), KeyAction::Pass); }
#[test] fn resync_clears_stuck_modifiers() {
    let mut k = m(&["q"]); k.on_key(0x5B, true, false);         // Win down, up never seen (Win+L)
    k.resync(|_| false);
    assert_eq!(k.on_key(0x51, true, false), KeyAction::Fire { combo: parse_combo("q").unwrap(), mask_menu: false });
}
#[test] fn matcher_is_pure_and_fast() {
    let mut k = m(&["Mod1+Return", "Mod1+j", "Mod1+k"]);
    let t = std::time::Instant::now();
    for i in 0..10_000u32 { k.on_key(0x41 + (i % 26) as u16, i % 2 == 0, false); }
    assert!(t.elapsed() < std::time::Duration::from_millis(50));
}
```

- [ ] **Step 2: Run tests** — Expected: FAIL.
- [ ] **Step 3: Implement `keyboard.rs`.**
- [ ] **Step 4: Run tests** — Expected: PASS.
- [ ] **Step 5: Commit** `feat(platform): keyboard matcher and low-level hook`

---

### Task 7: App, reconcile, startup — a working tiling WM

**Files:**
- Create: `crates/i3win/src/reconcile.rs`, `crates/i3win/src/app.rs`; Modify: `main.rs`, `mainloop.rs` (dispatch `WM_APP_EVENTS`, `WM_APP_KEY`, `WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE` with `wParam == SPI_SETWORKAREA (0x2F)` → `app.refresh_outputs()`)

**Interfaces:**
- Consumes: everything above; `Wm`, `Effect`, `Arrangement`, `floating_frame_from_client`.
- Produces:
  - `reconcile.rs` (pure):
    - `#[derive(Debug, PartialEq, Eq)] pub enum WinOp { Show(WindowId), Hide(WindowId), Place { id: WindowId, rect: Rect, raise: bool } }`
    - `#[derive(Default)] pub struct Applied { pub rects: HashMap<WindowId, Rect>, pub hidden: HashSet<WindowId> }`
    - `pub fn plan(applied: &mut Applied, arr: &Arrangement, managed: &HashSet<WindowId>) -> Vec<WinOp>`: for each placement: if it is in `applied.hidden` → `Show` and remove it from hidden; if `applied.rects.get(id) != Some(rect)` → `Place { raise: placement.floating }` and record it. For each `arr.hidden` id not yet in `applied.hidden` → `Hide`, insert it, remove it from `rects`. Finally drop `rects`/`hidden` entries whose id is not in `managed`. Order: all `Hide`s first, then `Show`s, then `Place`s (hiding first avoids flashing).
    - `pub fn invalidate(applied: &mut Applied, id: WindowId)` — removes the rect so the next plan re-places it.
  - `app.rs`:
    - `pub struct App { pub wm: Wm, applied: Applied, tracker: HideTracker, store: HiddenStore, config_path: PathBuf, last_combos: Vec<KeyCombo> }`
    - `pub fn App::start(config_path: PathBuf) -> App` — loads the config (on error: `Config::default()` plus the default config text), `monitors::enumerate()`, `Wm::new`, manages existing windows (`enumerate_top_level()` reversed so the topmost is managed last; classify; `info`), then `on_focus_changed(foreground)` if managed, queues the `exec` and `exec_always` commands (`Effect::Exec`), logs every `config.warnings` entry.
    - `pub fn on_event(&mut self, ev: PlatformEvent)`, `pub fn on_binding(&mut self, combo: KeyCombo)`, `pub fn on_main_msg(&mut self, msg: MainMsg)`, `pub fn refresh_outputs(&mut self)`, `pub fn flush(&mut self)`, `pub fn shutdown(&mut self)`
    - `pub(crate) fn decide(ev: PlatformEvent, managed: bool, expected_hide: bool) -> EventDecision` (pure), with `#[derive(Debug, PartialEq, Eq)] pub enum EventDecision { Ignore, TryManage, Unmanage, Focus, Title, MoveSizeEnd }`

`decide` table:
| event | managed | expected_hide | decision |
|---|---|---|---|
| Shown | false | – | TryManage |
| Shown | true | – | Ignore |
| TitleChanged | true | – | Title |
| TitleChanged | false | – | TryManage |
| Hidden | true | true | Ignore (the tracker already consumed it) |
| Hidden | true | false | Unmanage |
| Hidden | false | – | Ignore |
| Destroyed / Minimized | true | – | Unmanage |
| Destroyed / Minimized | false | – | Ignore |
| Restored | false | – | TryManage |
| Restored | true | – | Ignore |
| Foreground | true | – | Focus |
| Foreground | false | – | TryManage |
| MoveSizeEnd | true | – | MoveSizeEnd |
| MoveSizeEnd | false | – | Ignore |

`on_event` does: `expected_hide = matches!(ev, Hidden(_)) && tracker.consume(id, now)`; then `decide`. TryManage: `classify(props)`; Tile/Float → `wm.manage(info(id, Float))`. Unmanage: `wm.unmanage`, `tracker.forget`, `store.remove`. Focus: `wm.on_focus_changed`. Title: `wm.on_title_changed(id, title)`. MoveSizeEnd: if floating → `wm.on_floating_moved(id, floating_frame_from_client(&cfg, scale_of_output, frame_rect(id)))`; else `reconcile::invalidate` (it snaps back on flush). Every handler ends with `flush()`, except that `mainloop` calls `flush()` once after draining a whole batch of events.

`flush()`:
1. For each `wm.take_effects()`: `Exec(c)` → spawn `cmd.exe` with `raw_arg(format!("/C start \"\" {c}"))`, `creation_flags(CREATE_NO_WINDOW = 0x0800_0000)`, `current_dir(%USERPROFILE%)`, log errors. `Close(w)` → `window::close`. `Focus(w)` → `window::focus`. `Exit` → `mainloop::post(MainMsg::Shutdown)`. `Reload` → reread and parse the config; replace via `wm.reload_config`; queue `exec_always`; log warnings. `Ipc(_)` → ignore for now (Plan 4).
2. `arr = wm.arrange()`; `ops = reconcile::plan(...)`; apply: `Hide(id)` → if `window::hide(id)` returned true, `tracker.expect(id, now)`; `store.add(id)` either way. `Show(id)` → `window::show(id)`, `store.remove(id)`. `Place` → `window::place`.
3. If `wm.active_combos() != last_combos` → `keyboard::set_combos`, store them.
4. (Task 8 adds decoration reconcile here; Plan 3 adds bars.)

`main.rs` startup becomes: … single instance → `for id in hidden::take_previous(&hidden_path()) { if is_window(id) { show(id) } }` → `create_main_window` → `App::start` → store it in `APP` → `events::install()` → `keyboard::install()` → `flush()` → `run()` → `shutdown()` (uninstall hooks, `restore_all_hidden()`, delete hidden.txt).

- [ ] **Step 1: Write failing tests**

`reconcile.rs`:
```rust
fn arr(windows: Vec<(isize, Rect, bool)>, hidden: Vec<isize>) -> Arrangement {
    Arrangement { windows: windows.into_iter().map(|(i, r, f)| WindowPlacement { id: WindowId(i), rect: r, floating: f, fullscreen: false }).collect(), hidden: hidden.into_iter().map(WindowId).collect(), decos: vec![], fullscreen_outputs: vec![] }
}
fn managed(ids: &[isize]) -> HashSet<WindowId> { ids.iter().map(|&i| WindowId(i)).collect() }
#[test] fn first_plan_places_everything_and_hides() {
    let mut a = Applied::default();
    let ops = plan(&mut a, &arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![2]), &managed(&[1, 2]));
    assert_eq!(ops, vec![WinOp::Hide(WindowId(2)), WinOp::Place { id: WindowId(1), rect: Rect::new(0, 0, 10, 10), raise: false }]);
}
#[test] fn unchanged_plan_is_empty() {
    let mut a = Applied::default(); let x = arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![2]);
    plan(&mut a, &x, &managed(&[1, 2])); assert!(plan(&mut a, &x, &managed(&[1, 2])).is_empty());
}
#[test] fn unhide_shows_then_places() {
    let mut a = Applied::default();
    plan(&mut a, &arr(vec![], vec![1]), &managed(&[1]));
    let ops = plan(&mut a, &arr(vec![(1, Rect::new(0, 0, 5, 5), true)], vec![]), &managed(&[1]));
    assert_eq!(ops, vec![WinOp::Show(WindowId(1)), WinOp::Place { id: WindowId(1), rect: Rect::new(0, 0, 5, 5), raise: true }]);
}
#[test] fn invalidate_forces_replace() {
    let mut a = Applied::default(); let x = arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![]);
    plan(&mut a, &x, &managed(&[1])); invalidate(&mut a, WindowId(1));
    assert_eq!(plan(&mut a, &x, &managed(&[1])).len(), 1);
}
#[test] fn forgets_unmanaged() { let mut a = Applied::default(); plan(&mut a, &arr(vec![], vec![1]), &managed(&[1])); plan(&mut a, &arr(vec![], vec![]), &managed(&[])); assert!(a.hidden.is_empty()); }
```

`app.rs`:
```rust
#[test] fn hidden_expected_is_ignored() { assert_eq!(decide(PlatformEvent::Hidden(WindowId(1)), true, true), EventDecision::Ignore); }
#[test] fn hidden_unexpected_unmanages() { assert_eq!(decide(PlatformEvent::Hidden(WindowId(1)), true, false), EventDecision::Unmanage); }
#[test] fn title_change_on_unmanaged_retries_manage() { assert_eq!(decide(PlatformEvent::TitleChanged(WindowId(1)), false, false), EventDecision::TryManage); }
#[test] fn minimize_unmanages_restore_manages() { assert_eq!(decide(PlatformEvent::Minimized(WindowId(1)), true, false), EventDecision::Unmanage); assert_eq!(decide(PlatformEvent::Restored(WindowId(1)), false, false), EventDecision::TryManage); }
#[test] fn movesize_only_for_managed() { assert_eq!(decide(PlatformEvent::MoveSizeEnd(WindowId(1)), false, false), EventDecision::Ignore); }
```

- [ ] **Step 2: Run tests** — Expected: FAIL.
- [ ] **Step 3: Implement `reconcile.rs`, `app.rs`, and the wiring in `main.rs`/`mainloop.rs`.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Manual check** (write the results in the commit body). Build with `cargo build -p i3win`. Close other apps' windows you don't want tiled. Run `target\debug\i3win.exe`.
  - Open 3 Notepads with `start notepad` ×3 → they tile side by side across the monitor work area (taskbar not covered). There are no title bars yet (Task 8).
  - `Alt+j` / `Alt+;` move focus. `Alt+Shift+j` swaps windows. `Alt+v` then `Alt+Return` opens a terminal below the focused window.
  - `Alt+2` → Notepads disappear; `Alt+1` → they come back in the same layout. The Start menu and Notepad's menu bar must not open on Alt release.
  - `Alt+Shift+q` closes the focused Notepad (the save prompt appears floating if the file is dirty).
  - `Alt+Shift+e` exits; every window is visible again.
  - Run i3win, switch to an empty workspace 2, then `taskkill /IM i3win.exe /F`. Hidden Notepads stay hidden. Start i3win again → they reappear.
- [ ] **Step 6: Commit** `feat(platform): app loop, reconcile and startup — tiling works`

---

### Task 8: Decorations — title bars, borders, tab/stack bars

**Files:**
- Create: `crates/i3win/src/win/deco.rs`, `crates/i3win/src/win/gdi.rs`; Modify: `win/mod.rs`, `app.rs` (call `deco::reconcile` in `flush`, handle `MainMsg::FocusNode` → `wm.focus_node(n)` then flush), `mainloop.rs`

**Interfaces:**
- Consumes: `arrange::{Deco, DecoState, Tab}`, `config::{ClientColors, ColorClass, Color, Font}`, `mainloop::post(MainMsg::FocusNode)`.
- Produces:
  - `gdi.rs`: `pub fn colorref(c: Color) -> COLORREF` (`r | g<<8 | b<<16`), `pub fn colors_for(state: DecoState, cc: &ClientColors) -> ColorClass` (pure), `pub fn font(family: &str, size_pt: f32, scale: f32) -> HFONT` (thread-local cache keyed by `(family, (size_pt*10) as i32, (scale*100) as i32)`; `CreateFontW` height `-((size_pt*96/72*scale).round())`, weight 400, `CLEARTYPE_QUALITY`), `pub fn paint_buffered(hwnd, f: impl FnOnce(HDC, Rect))` (BeginPaint → compatible DC + bitmap of the client size → f → BitBlt → cleanup → EndPaint).
  - `deco.rs`:
    - `pub struct DecoData { pub deco: Deco, pub colors: ClientColors, pub font: Font, pub scale: f32 }` — stored boxed in each deco window's `GWLP_USERDATA`, freed on `WM_NCDESTROY`.
    - `pub struct Decos { windows: HashMap<NodeId, (HWND, DecoData)> }` with `new()`, `reconcile(&mut self, arr: &Arrangement, cfg: &Config, scale_of: impl Fn(Rect) -> f32)`, `destroy_all(&mut self)`.
    - `pub fn deco_key(d: &Deco) -> NodeId` (pure), `pub fn tab_at(tabs: &[Tab], origin: (i32, i32), x: i32, y: i32) -> Option<NodeId>` (pure: screen point → tab node; `origin` is the deco window's screen top-left and tab rects are screen coordinates).

Reconcile rules:
- Class `i3win_deco`, `CS_HREDRAW | CS_VREDRAW`, null background brush. Create with `WS_POPUP` (not visible yet), `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`.
- For each deco in `arr.decos`: get or create its window by key. If the stored data changed, replace it and `InvalidateRect`. Position:
  - `Frame`: `SetWindowPos(deco, HWND(above), rect..., SWP_NOACTIVATE | SWP_SHOWWINDOW)` — this puts the deco directly BELOW its client window in the z-order (the client covers the middle; the title strip and border edges show around it). Do this every flush, since focus changes reorder clients.
  - `TabBar`: on creation `SetWindowPos(HWND_TOP, ..., SWP_NOACTIVATE | SWP_SHOWWINDOW)`; afterwards only when the rect changed, with `SWP_NOZORDER`.
- Keys not in `arr.decos` → `DestroyWindow`.
- `scale_of(rect)` = the scale of the output containing the rect's center (App passes a closure over `wm` outputs).

Painting (`WM_PAINT`, via `paint_buffered`; all coordinates are relative to the deco window):
- Frame: fill the whole client area with `child_border`. If `title_rect` is set: the title area (`0, 0, w, title_rect.h`) is filled with `background`, then a 1 px outline in `border`, then the title with `DrawTextW` in `text` color, `DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | DT_LEFT`, inset horizontally by `round(4*scale)` px, `SetBkMode(TRANSPARENT)`.
- TabBar: for each tab (rect relative = tab.rect minus the deco's rect origin): background, 1 px border outline, title text (same flags).
- Colors via `colors_for(state)`: Focused → `focused`, FocusedInactive → `focused_inactive`, Unfocused → `unfocused`, Urgent → `urgent`.
- `WM_LBUTTONDOWN`: Frame → `post(FocusNode(key))`; TabBar → `tab_at` with the cursor's screen position → `post(FocusNode(node))`. `WM_MOUSEACTIVATE` → return `MA_NOACTIVATE`.
- `WM_ERASEBKGND` → return 1.

`App::shutdown` also calls `decos.destroy_all()`.

- [ ] **Step 1: Write failing tests**

```rust
// gdi.rs
#[test] fn colorref_packs_bgr() { assert_eq!(colorref(Color(0x11, 0x22, 0x33)).0, 0x0033_2211); }
#[test] fn state_to_color_class() { let cc = i3win_core::config::Config::default().colors; assert_eq!(colors_for(DecoState::Urgent, &cc), cc.urgent); assert_eq!(colors_for(DecoState::FocusedInactive, &cc), cc.focused_inactive); }
// deco.rs
#[test] fn tab_hit_testing() {
    let tabs = vec![Tab { title: "a".into(), state: DecoState::Unfocused, rect: Rect::new(100, 0, 50, 20), node: NodeId(1) }, Tab { title: "b".into(), state: DecoState::Focused, rect: Rect::new(150, 0, 50, 20), node: NodeId(2) }];
    assert_eq!(tab_at(&tabs, (100, 0), 160, 10), Some(NodeId(2)));
    assert_eq!(tab_at(&tabs, (100, 0), 99, 10), None);
}
#[test] fn key_of_deco() { let d = Deco::TabBar { key: NodeId(9), rect: Rect::default(), vertical: false, tabs: vec![] }; assert_eq!(deco_key(&d), NodeId(9)); }
```

- [ ] **Step 2: Run tests** — Expected: FAIL.
- [ ] **Step 3: Implement `gdi.rs`, `deco.rs`, and the App and mainloop wiring.**
- [ ] **Step 4: Run** `cargo test --workspace` — Expected: PASS.
- [ ] **Step 5: Manual check.** Run i3win and open 3 Notepads → each has an i3-blue title bar above it showing its title (focused = `#285577`, others `#222222`), with a 2 px border. `Alt+w` → a tab row appears with 3 tabs. Clicking a tab switches to it. `Alt+s` → stacked rows. `Alt+e` → back to split. `Alt+Shift+space` → the Notepad floats centered, with a title bar that follows it. `Alt+2`/`Alt+1` → decorations disappear and reappear with their windows. Typing in Notepad changes its title live.
- [ ] **Step 6: Commit** `feat(platform): i3-style decorations`

---

### Task 9: Crash safety and end-to-end smoke test

**Files:**
- Modify: `main.rs`; Create: `crates/i3win/tests/smoke.rs`; Modify: `crates/i3win/Cargo.toml` (`[dev-dependencies] windows = { version = "0.58", features = [same list] }`)

**Interfaces:**
- Consumes: `hidden::restore_all_hidden`.
- Produces: no new API.

Behavior to add in `main.rs`:
- `std::panic::set_hook`: log the panic (`log::error!`), call `hidden::restore_all_hidden()`, then run the default hook.
- `SetConsoleCtrlHandler(Some(handler), true)`: for `CTRL_C_EVENT`, `CTRL_BREAK_EVENT`, `CTRL_CLOSE_EVENT`, `CTRL_LOGOFF_EVENT` and `CTRL_SHUTDOWN_EVENT` → `restore_all_hidden()`, then `mainloop::post(MainMsg::Shutdown)`, return TRUE.
- Main wndproc: `WM_ENDSESSION` with `wParam != 0` → `restore_all_hidden()`.

`tests/smoke.rs` (`#[ignore]`, run with `cargo test -p i3win --test smoke -- --ignored --test-threads=1`):
1. Write a temp config: `default_border none\nbindsym Mod1+Shift+F12 exit\n` (no bar).
2. Spawn `env!("CARGO_BIN_EXE_i3win")` with `--config <temp>`; wait 1 s.
3. Spawn two `notepad.exe` and keep their PIDs; poll up to 5 s until each PID has a visible top-level window (`EnumWindows` + `GetWindowThreadProcessId`).
4. Wait 1 s more, then read both visible frames (`DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)`).
5. Assert: the frames don't overlap; the left one's right edge is within 2 px of the right one's left edge; each width is within 10 px of half the primary monitor's work-area width; both tops equal the work-area top.
6. Clean up regardless of outcome: kill both Notepads and i3win (`Child::kill`).

- [ ] **Step 1: Write the smoke test** as above.
- [ ] **Step 2: Run** `cargo test -p i3win --test smoke -- --ignored --test-threads=1` — Expected: PASS (it exercises Tasks 1–7; if it fails, fix the platform code, not the test's tolerances).
- [ ] **Step 3: Add the panic hook, console handler and `WM_ENDSESSION` handling.**
- [ ] **Step 4: Manual check.** Add a debug-only trigger: in `#[cfg(debug_assertions)]` builds, if env `I3WIN_PANIC_AFTER_SECS=N` is set, spawn a thread that sleeps N seconds and then `mainloop::post(MainMsg::DebugPanic)` (a new `#[cfg(debug_assertions)]` variant); the main thread `panic!`s when it handles it. Run with `I3WIN_PANIC_AFTER_SECS=15`, open Notepads, switch to workspace 2 before 15 s → after the panic all Notepads reappear and the log shows the panic message. Run `i3win.exe` from a console, hide windows, press Ctrl+C → windows reappear.
- [ ] **Step 5: Run** `cargo test --workspace` and clippy — Expected: PASS.
- [ ] **Step 6: Commit** `feat(platform): crash safety and smoke test`
