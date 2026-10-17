# Plan 5: Window Commands, Config and Defaults Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make i3win's defaults match i3 and add the i3 window commands and config options v1 skipped (marks, scratchpad, sticky, border/title commands, edge/smart borders and gaps, runtime gaps, rename, focus options, criteria, move/resize forms, `--release`, `bindcode`).

**Architecture:** Almost everything lands in the pure `i3win-core` crate (config parsing, `Command` parsing, `Wm` operations, `arrange()`), unit-tested with `testutil`. The Win32 crate `i3win` only gets small plumbing: per-side borders and icons in `win/deco.rs`, the keyboard matcher for `--release`/`bindcode`, the pointer position and computer name handed to `Wm`.

**Tech Stack:** Rust 2021 stable, MSVC; `windows` 0.58 (pinned); `regex`, `serde_json` in core.

**Spec:** `docs/superpowers/specs/2026-10-17-i3win-v2-design.md` (sections "Defaults", "Window commands and config", "IPC additions", "Compatibility rules"), building on `docs/superpowers/specs/2026-10-16-i3win-design.md`.

## Global Constraints

- `i3win-core` has no `windows` dependency; every new piece of logic that can live there does, with unit tests.
- `windows` crate stays at `0.58`.
- The tree, `Wm`, `Effect`, `Arrangement` and config structs may be extended and refactored; v1 unit tests may be updated only where a v2 behavior intentionally changes them, never deleted to make a change pass.
- IPC JSON stays i3-shaped: fields are only added, never renamed or removed.
- `hidden.txt` stays `hwnd pid` per line.
- Every v1 config stays valid with the same meaning (except bar position and font defaults, which the spec changes).
- An existing config file is never rewritten.
- Bad config lines are logged and skipped, never a crash. Commands with bad arguments return `{"success":false,"error":...}`.
- Never make a synchronous cross-process call that can block on a hung window (use `GetClassLongPtrW`, `SendMessageCallbackW`, never `SendMessageW`/`SendMessageTimeoutW` to other processes from the main thread).
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Run `cargo test --workspace`, `cargo clippy --workspace -- -D warnings` and `cargo fmt --all -- --check` before every commit; all must pass.

## Review Focus

1. A scratchpad window must come back on exit or crash: it is hidden via the same `Arrangement.hidden` path as workspace hides, so `hidden.txt` records it (Task 8 test `scratchpad_windows_are_in_arrangement_hidden`).
2. The `__i3` pseudo-output must never show up as a real monitor: not in `Wm::outputs()`, `GET_OUTPUTS`, `GET_WORKSPACES`, bars, `focus output`, or `workspace next` (Task 8 test `scratch_output_is_invisible_everywhere`).
3. Renaming to a name that already exists must fail without changing anything (Task 10 test `rename_to_existing_name_fails`).
4. `focus_on_window_activation` must never block a user's click on a visible window: it only applies to windows on hidden workspaces (Task 12 test `activation_none_still_allows_visible_focus`).
5. Per-side border hiding must not leave a gap or overlap between a window and its title bar (Task 4 test `hidden_edge_borders_shift_client_rect_exactly`).

---

### Task 1: New global config options

**Files:**
- Modify: `crates/core/src/config.rs`

**Interfaces:**
- Produces in `i3win_core::config` (all `Clone, Copy, Debug, PartialEq, Eq` enums), with `Config` fields and defaults:

```rust
pub enum MouseWarping { Output, Container, None }            // mouse_warping, default Output
pub struct TilingDrag { pub modifier: bool, pub titlebar: bool } // tiling_drag, default both true; "off" = both false
pub enum FocusWrapping { Yes, No, Force, Workspace }          // focus_wrapping, default Yes
pub enum PopupDuringFullscreen { Smart, Ignore, LeaveFullscreen } // default Smart
pub enum FocusOnActivation { Smart, Urgent, Focus, None }     // focus_on_window_activation, default Smart
pub enum HideEdgeBorders { None, Vertical, Horizontal, Both, Smart, SmartNoGaps } // default None
pub enum SmartBorders { Off, On, NoGaps }                     // smart_borders, default Off
pub enum SmartGaps { Off, On, InverseOuter }                  // smart_gaps, default Off
// Config fields:
pub focus_follows_mouse: bool,            // default true; "yes"/"no"
pub mouse_warping: MouseWarping,
pub floating_modifier: Option<Modifiers>, // default None; value parsed like a combo's modifiers ("Mod1", "$mod" after expansion, "Mod4+Shift")
pub tiling_drag: TilingDrag,
pub show_marks: bool,                     // default true
pub focus_wrapping: FocusWrapping,
pub workspace_auto_back_and_forth: bool,  // default false
pub popup_during_fullscreen: PopupDuringFullscreen,
pub focus_on_window_activation: FocusOnActivation,
pub floating_minimum_size: (i32, i32),    // default (75, 50); "WxH", "-1" = no limit -> (0,0), "0" = default
pub floating_maximum_size: (i32, i32),    // default (0, 0) = unlimited; "-1 x -1" = unlimited, "0 x 0" = default
pub hide_edge_borders: HideEdgeBorders,
pub smart_borders: SmartBorders,
pub smart_gaps: SmartGaps,
pub no_focus: Vec<Criteria>,
```

- `tiling_drag` accepts `off`, or any of `modifier`, `titlebar` in either order. `smart_borders`/`smart_gaps` accept `on|off|no_gaps|inverse_outer` per their enum; `smart_borders yes` = On. `no_focus [criteria]` parses like `for_window`'s criteria. `floating_minimum_size 75 x 50` and `75x50` both parse.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn new_options_defaults() {
    let c = parse_config("");
    assert!(c.focus_follows_mouse);
    assert_eq!(c.mouse_warping, MouseWarping::Output);
    assert_eq!(c.floating_modifier, None);
    assert_eq!(c.tiling_drag, TilingDrag { modifier: true, titlebar: true });
    assert!(c.show_marks);
    assert_eq!(c.focus_wrapping, FocusWrapping::Yes);
    assert!(!c.workspace_auto_back_and_forth);
    assert_eq!(c.popup_during_fullscreen, PopupDuringFullscreen::Smart);
    assert_eq!(c.focus_on_window_activation, FocusOnActivation::Smart);
    assert_eq!(c.floating_minimum_size, (75, 50));
    assert_eq!(c.floating_maximum_size, (0, 0));
    assert_eq!(c.hide_edge_borders, HideEdgeBorders::None);
    assert_eq!(c.smart_borders, SmartBorders::Off);
    assert_eq!(c.smart_gaps, SmartGaps::Off);
}
#[test]
fn new_options_parse() {
    let c = parse_config(
        "set $mod Mod4\nfocus_follows_mouse no\nmouse_warping none\nfloating_modifier $mod\n\
         tiling_drag titlebar\nshow_marks no\nfocus_wrapping workspace\n\
         workspace_auto_back_and_forth yes\npopup_during_fullscreen leave_fullscreen\n\
         focus_on_window_activation urgent\nfloating_minimum_size 100 x 80\n\
         floating_maximum_size 800x600\nhide_edge_borders smart_no_gaps\n\
         smart_borders no_gaps\nsmart_gaps inverse_outer\nno_focus [class=\"^x$\"]\n",
    );
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    assert!(!c.focus_follows_mouse);
    assert_eq!(c.mouse_warping, MouseWarping::None);
    assert_eq!(c.floating_modifier, Some(Modifiers::WIN));
    assert_eq!(c.tiling_drag, TilingDrag { modifier: false, titlebar: true });
    assert!(!c.show_marks);
    assert_eq!(c.focus_wrapping, FocusWrapping::Workspace);
    assert!(c.workspace_auto_back_and_forth);
    assert_eq!(c.popup_during_fullscreen, PopupDuringFullscreen::LeaveFullscreen);
    assert_eq!(c.focus_on_window_activation, FocusOnActivation::Urgent);
    assert_eq!(c.floating_minimum_size, (100, 80));
    assert_eq!(c.floating_maximum_size, (800, 600));
    assert_eq!(c.hide_edge_borders, HideEdgeBorders::SmartNoGaps);
    assert_eq!(c.smart_borders, SmartBorders::NoGaps);
    assert_eq!(c.smart_gaps, SmartGaps::InverseOuter);
    assert_eq!(c.no_focus.len(), 1);
    assert_eq!(parse_config("tiling_drag off\n").tiling_drag, TilingDrag { modifier: false, titlebar: false });
    assert_eq!(parse_config("floating_minimum_size -1 x -1\n").floating_minimum_size, (0, 0));
}
#[test]
fn bad_new_option_values_warn_and_keep_default() {
    let c = parse_config("focus_wrapping sideways\nmouse_warping x\n");
    assert_eq!(c.warnings.len(), 2);
    assert_eq!(c.focus_wrapping, FocusWrapping::Yes);
}
```

- [ ] **Step 2: Run** `cargo test -p i3win-core config::` — FAIL (types missing).
- [ ] **Step 3: Implement** the enums, fields and parsing branches in `parse_line`'s keyword match.
- [ ] **Step 4: Run** `cargo test --workspace` — PASS.
- [ ] **Step 5: Commit** `feat(config): i3 options (mouse, focus, borders, gaps, sizes, no_focus)`.

---

### Task 2: i3 defaults, default config file, font family names

**Files:**
- Modify: `crates/core/src/config.rs` (BarConfig default, Font default, new `resolve_font_family`)
- Modify: `crates/core/src/default_config` (rewritten)
- Modify: `crates/i3win/src/win/gdi.rs` (`font()` maps the family)
- Modify: `docs/superpowers/specs/2026-10-16-i3win-design.md` line "i3win default bar position is `top`..." → replace with "Default bar position is `bottom`, as in i3 (v2 spec)."

**Interfaces:**
- Produces: `pub fn resolve_font_family(name: &str) -> &str` in `i3win_core::config`.

- [ ] **Step 1: Write the failing tests** (in `config.rs` tests)

```rust
#[test]
fn defaults_match_i3() {
    let c = parse_config("bar {\n}\n");
    assert_eq!(c.bar.as_ref().unwrap().position, BarPosition::Bottom);
    assert_eq!(c.font, Font { family: "monospace".into(), size_pt: 8.0 });
    assert_eq!(c.default_border, Border::Normal(2));
}
#[test]
fn font_family_aliases() {
    assert_eq!(resolve_font_family("monospace"), "Consolas");
    assert_eq!(resolve_font_family("Monospace"), "Consolas");
    assert_eq!(resolve_font_family("sans"), "Segoe UI");
    assert_eq!(resolve_font_family("sans-serif"), "Segoe UI");
    assert_eq!(resolve_font_family("serif"), "Times New Roman");
    assert_eq!(resolve_font_family("DejaVu Sans Mono"), "DejaVu Sans Mono");
}
#[test]
fn default_config_text_is_i3s() {
    let t = default_config_text(true);
    for line in [
        "set $mod Mod1",
        "font pango:monospace 8",
        "floating_modifier $mod",
        "tiling_drag modifier titlebar",
        "focus_follows_mouse no",
        "bindsym $mod+Return exec wt",
        "bindsym $mod+Shift+e exit",
        "bindsym $mod+r mode \"resize\"",
        "status_command i3win-status",
    ] {
        assert!(t.contains(line), "missing: {line}");
    }
    assert!(!t.contains("position"));
    assert!(default_config_text(false).contains("bindsym $mod+Return exec cmd"));
    let c = parse_config(&t);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}
```

- [ ] **Step 2: Run** `cargo test -p i3win-core config::` — expect `defaults_match_i3`, `font_family_aliases`, `default_config_text_is_i3s` to FAIL.

- [ ] **Step 3: Implement.**
  - `BarConfig::default().position = BarPosition::Bottom`; the no-`font`-line default is `Font { family: "monospace", size_pt: 8.0 }`.
  - `resolve_font_family`: case-insensitive `monospace`→`Consolas`, `sans`/`sans-serif`→`Segoe UI`, `serif`→`Times New Roman`, else the input unchanged.
  - `gdi::font(family, ..)` calls `resolve_font_family(family)` before creating the HFONT.
  - Rewrite `default_config` to follow i3 4.22's `/etc/i3/config` in order, keeping its comments, with only these substitutions: `i3-sensible-terminal` → `wt` (the existing `has_wt` replacement to `cmd` stays); `dmenu_run` → the existing `explorer.exe shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}` Run dialog line; `i3status` → `i3win-status`; drop the `dex`, `xss-lock`, `nm-applet`, `$refresh_i3status` and `pactl` lines; `$mod+Shift+e` is `exit` (i3win has no nagbar). Add exactly one line not in i3's file, after `tiling_drag modifier titlebar`, with a comment: `focus_follows_mouse no`. The `bar {}` block has only `status_command i3win-status`.
  - **Ruling for the spec's Defaults section:** i3 4.22's default config has no scratchpad bindings, so none are added. The spec's sentence listing `$mod+Shift+minus move scratchpad` / `$mod+minus scratchpad show` as gained lines is corrected in this task: edit that sentence in `docs/superpowers/specs/2026-10-17-i3win-v2-design.md` to drop the scratchpad bindings.
  - `floating_modifier`, `tiling_drag`, `focus_follows_mouse` parse without warnings since Task 1.

- [ ] **Step 4: Run** `cargo test --workspace` — PASS. Fix any v1 test that asserted `BarPosition::Top` or `Segoe UI` as the default (update to the new default, don't delete).

- [ ] **Step 5: Commit** `feat(config): i3 defaults (bar bottom, monospace 8) and i3's default config`.

---

### Task 3: Gaps model (all sides, per-workspace)

**Files:**
- Modify: `crates/core/src/config.rs`, `crates/core/src/arrange.rs`, `crates/core/src/tree.rs`

**Interfaces:**
- Produces:

```rust
// config.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Gaps { pub inner: i32, pub top: i32, pub right: i32, pub bottom: i32, pub left: i32 }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GapsOverride { pub inner: Option<i32>, pub top: Option<i32>, pub right: Option<i32>, pub bottom: Option<i32>, pub left: Option<i32> }
impl Gaps { pub fn with(self, o: &GapsOverride) -> Gaps }   // each Some field replaces
// Config: replace `gaps_inner`/`gaps_outer` with
pub gaps: Gaps,
pub workspace_gaps: Vec<(String, GapsOverride)>,
// tree.rs Node gains
pub gaps: GapsOverride,   // runtime per-workspace override (Task 5 writes it), default all None
// Wm (wm.rs or arrange.rs):
pub fn effective_gaps(&self, ws: NodeId) -> Gaps   // config.gaps.with(config workspace_gaps for ws name).with(&node.gaps)
```

- Config forms (i3 4.22): `gaps inner|outer|horizontal|vertical|top|right|bottom|left N`. `outer N` sets top/right/bottom/left; `horizontal N` sets left and right; `vertical N` sets top and bottom. Same after `workspace NAME gaps ...` into `workspace_gaps` (one entry per workspace name, merged).
- `arrange` uses `effective_gaps(ws)` instead of `gaps_inner`/`gaps_outer`: inner as before; the workspace area is inset by `top/right/bottom/left` (+ the inner half-gap lead/trail exactly as v1 computes `lead`/`trail` per side). `arrange(tree, config, focused)` keeps its signature; it computes the effective gaps from `config` and the workspace node (move `effective_gaps` logic into a free fn `effective_gaps(tree, config, ws)` used by both).

- [ ] **Step 1: Write the failing tests**

```rust
// config.rs
#[test]
fn gaps_forms() {
    let c = parse_config("gaps inner 10\ngaps outer 5\ngaps horizontal 7\ngaps top 1\nworkspace 2 gaps inner 0\nworkspace 2 gaps left 3\n");
    assert_eq!(c.gaps, Gaps { inner: 10, top: 1, right: 7, bottom: 5, left: 7 });
    assert_eq!(c.workspace_gaps, vec![("2".to_string(), GapsOverride { inner: Some(0), left: Some(3), ..Default::default() })]);
}
// arrange.rs
#[test]
fn per_side_outer_gaps_and_workspace_override() {
    let mut cfg = parse_config("gaps top 20\ngaps left 10\nworkspace 2 gaps top 0\ndefault_border none\n");
    cfg.bar = None;
    let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    let r = wm.arrange().windows[0].rect;
    assert_eq!((r.x, r.y, r.w, r.h), (10, 20, 990, 480));
    wm.run_command("workspace 2");
    wm.manage(win(2));
    let r2 = wm.arrange().windows.iter().find(|p| p.id == WindowId(2)).unwrap().rect;
    assert_eq!((r2.x, r2.y), (10, 0));
}
```

- [ ] **Step 2: Run** `cargo test -p i3win-core` — FAIL.
- [ ] **Step 3: Implement.** Update every use of `gaps_inner`/`gaps_outer` (core arrange, IPC `get_config`-free; grep the workspace). v1 `gaps inner N`/`gaps outer N` tests keep their expected geometry.
- [ ] **Step 4: Run** `cargo test --workspace` — PASS.
- [ ] **Step 5: Commit** `feat(gaps): per-side and per-workspace gaps`.

---

### Task 4: Per-window border, title_format, title icons, edge and smart borders

**Files:**
- Modify: `crates/core/src/tree.rs` (Node fields), `crates/core/src/command.rs`, `crates/core/src/wm.rs` (dispatch), `crates/core/src/arrange.rs`
- Create: `crates/core/src/title.rs` (+ `pub mod title;` in `lib.rs`)
- Modify: `crates/i3win/src/win/deco.rs` (per-side borders, icon), `crates/i3win/src/app.rs` (set `wm.machine`)

**Interfaces:**
- Produces:

```rust
// tree.rs Node gains (defaults in new_node):
pub border: Option<Border>,        // per-window override of default_border/default_floating_border
pub title_format: Option<String>,
pub title_icon: Option<i32>,       // Some(padding_px) when title_window_icon is on
pub marks: Vec<String>,            // written by Task 6; declared here so title.rs can use it
// command.rs
pub enum BorderArg { Normal(Option<i32>), Pixel(Option<i32>), None, Toggle(Option<i32>) }
Command::Border(BorderArg)
Command::TitleFormat(String)
Command::TitleWindowIcon(TitleIconArg)    // enum TitleIconArg { On, Off, Toggle, Padding(i32) }
// title.rs
pub fn format_title(format: Option<&str>, title: &str, class: &str, instance: &str, machine: &str, marks: &[String], show_marks: bool) -> String
// wm.rs
pub machine: String   // computer name; platform sets it at start (App::start: std::env::var("COMPUTERNAME"))
// arrange.rs: Deco::Frame changes `border: i32` to
borders: [i32; 4],    // left, top, right, bottom; with a normal (title) border, top is 0 and the title bar sits there
icon: Option<(WindowId, i32)>,   // window whose icon to draw, padding px
```

- Command forms (i3): `border normal [N]`, `border pixel [N]` (N default: keep the current width if the window had that style, else 2 for normal / 1 for pixel), `border none`, `border toggle [N]` cycles normal → pixel → none → normal. `title_format "%title (%class)"`. `title_window_icon on|off|toggle|padding Npx`.
- `format_title`: `None` format = `%title`. Replaces `%title %class %instance %machine` (unknown `%x` stay literal). If `show_marks` and marks non-empty, appends ` [m1] [m2]` (i3 shows each mark in brackets; marks starting with `_` are not shown).
- Border sides (`arrange::emit_window`): start from the style's width on all 4 sides, then:
  - `hide_edge_borders vertical|both` zeroes left/right sides that touch the workspace's tiling area edge; `horizontal|both` zeroes top/bottom; `smart` = zero all sides when the workspace has exactly one visible tiled window; `smart_no_gaps` = like `smart` but only when the effective inner gap is 0. "Touches the edge" = the frame side equals the tiling area side (the area after bar and outer gaps).
  - `smart_borders on` = like `hide_edge_borders smart`; `no_gaps` = like `smart_no_gaps`. Applies to tiled windows only; floating windows always keep their borders.
  - The title bar (normal border) is never removed by edge rules; only the 1-4 px border lines are.
  - Client rect = frame inset by `[left, top_or_title, right, bottom]`, so a hidden side gives the client that space.
- `smart_gaps on`: when the workspace has exactly one visible tiled window, inner and outer gaps are 0. `inverse_outer`: outer gaps are applied only when the workspace has exactly one tiled window.
- Platform: `deco.rs` draws each border side with its own width; when `icon` is set, draws the window's small icon (`GetClassLongPtrW(GCLP_HICONSM)`, falling back to `GCLP_HICON`; additionally request `WM_GETICON ICON_SMALL2` once per window via `SendMessageCallbackW` and cache the HICON the callback returns, keyed by WindowId) at the left of the title, vertically centered, `padding` px before the text, size = title height − 4 px.

- [ ] **Step 1: Write the failing tests**

```rust
// title.rs
#[test]
fn format_title_placeholders_and_marks() {
    assert_eq!(format_title(None, "T", "c", "i", "PC", &[], true), "T");
    assert_eq!(format_title(Some("%title (%class/%instance) @%machine %x"), "T", "c", "i", "PC", &[], true), "T (c/i) @PC %x");
    let marks = vec!["a".to_string(), "_hidden".to_string(), "b".to_string()];
    assert_eq!(format_title(None, "T", "c", "i", "PC", &marks, true), "T [a] [b]");
    assert_eq!(format_title(None, "T", "c", "i", "PC", &marks, false), "T");
}
// arrange.rs
#[test]
fn border_command_and_toggle() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("border pixel 3");
    let Deco::Frame { borders, title_rect, .. } = &wm.arrange().decos[0] else { panic!() };
    assert_eq!(*borders, [3, 3, 3, 3]);
    assert!(title_rect.is_none());
    wm.run_command("border toggle");
    assert!(wm.arrange().decos.is_empty()); // pixel -> none
    wm.run_command("border toggle");
    assert!(matches!(&wm.arrange().decos[0], Deco::Frame { title_rect: Some(_), .. }));
}
#[test]
fn title_format_applies_to_frame_title() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("title_format \"<%class> %title\"");
    let Deco::Frame { title, .. } = &wm.arrange().decos[0] else { panic!() };
    assert_eq!(title, "<c1> w1");
}
#[test]
fn hidden_edge_borders_shift_client_rect_exactly() {
    let mut cfg = parse_config("hide_edge_borders both\ndefault_border pixel 2\n");
    cfg.bar = None;
    let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    wm.manage(win(2));
    let a = wm.arrange();
    let w1 = a.windows.iter().find(|p| p.id == WindowId(1)).unwrap().rect;
    let w2 = a.windows.iter().find(|p| p.id == WindowId(2)).unwrap().rect;
    assert_eq!((w1.x, w1.y, w1.h), (0, 0, 500));      // outer sides hidden
    assert_eq!(w1.right() + 2 + 2, w2.x);              // inner sides keep 2 px each
    assert_eq!(w2.right(), 1000);
}
#[test]
fn smart_borders_only_with_single_window() {
    let mut cfg = parse_config("smart_borders on\n");
    cfg.bar = None;
    let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    let Deco::Frame { borders, .. } = &wm.arrange().decos[0] else { panic!() };
    assert_eq!(*borders, [0, 0, 0, 0]);
    wm.manage(win(2));
    let Deco::Frame { borders, title_rect, .. } = &wm.arrange().decos[0] else { panic!() };
    assert_eq!(*borders, [2, 0, 2, 2]);
    assert!(title_rect.is_some());
}
#[test]
fn smart_gaps_single_window_has_no_gaps() {
    let mut cfg = parse_config("gaps inner 10\ngaps outer 5\nsmart_gaps on\ndefault_border none\n");
    cfg.bar = None;
    let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    assert_eq!(wm.arrange().windows[0].rect, Rect::new(0, 0, 1000, 500));
    wm.manage(win(2));
    assert_ne!(wm.arrange().windows[0].rect.x, 0);
}
#[test]
fn title_window_icon_sets_deco_icon() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("title_window_icon padding 4px");
    let Deco::Frame { icon, .. } = &wm.arrange().decos[0] else { panic!() };
    assert_eq!(*icon, Some((WindowId(1), 4)));
}
```

(Use whatever helper the existing arrange tests use for `out`/`win`/`parse_config` imports.)

- [ ] **Step 2: Run** `cargo test -p i3win-core` — FAIL.
- [ ] **Step 3: Implement** core parts; then update `deco.rs` for `borders: [i32;4]` and `icon`, and set `wm.machine` in `App::start`. Tab titles (`Tab.title`) for window children also go through `format_title` with the child's `title_format` and marks.
- [ ] **Step 4: Run** `cargo test --workspace`, clippy, fmt — PASS. `cargo build --workspace` succeeds.
- [ ] **Step 5: Commit** `feat: border/title_format/title_window_icon commands, edge and smart borders, smart gaps`.

---

### Task 5: Runtime `gaps` command

**Files:** Modify `crates/core/src/command.rs`, `crates/core/src/wm.rs` (or new `crates/core/src/ops/gaps.rs`, registered in `ops/mod.rs`)

**Interfaces:**
- Consumes: `Gaps`, `GapsOverride`, `Node.gaps`, `effective_gaps` (Task 3).
- Produces: `Command::Gaps { which: GapsWhich, scope: GapsScope, op: GapsOp, px: i32 }` with
  `enum GapsWhich { Inner, Outer, Horizontal, Vertical, Top, Right, Bottom, Left }`, `enum GapsScope { Current, All }`, `enum GapsOp { Set, Plus, Minus, Toggle }`.
- Semantics (i3 4.22): `current` writes the focused workspace's `Node.gaps` override for the affected sides; `all` writes `config.gaps` for those sides and clears those sides from every workspace node override. `plus`/`minus` add to the current effective value; results clamp at 0. `toggle N`: if the current effective value of the side is 0 → set N, else set 0 (for multi-side `which`, decide by the first side: top for outer/vertical, left for horizontal).
- The command is a non-window command (add to `is_window_command`'s exclusions).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn gaps_command_current_and_all() {
    let mut wm = test_wm();
    assert!(wm.run_command("gaps inner current set 12")[0].success);
    let ws = wm.focused_workspace();
    assert_eq!(wm.effective_gaps(ws).inner, 12);
    wm.run_command("gaps outer current plus 4");
    assert_eq!(wm.effective_gaps(ws), Gaps { inner: 12, top: 4, right: 4, bottom: 4, left: 4 });
    wm.run_command("gaps horizontal current minus 10");
    assert_eq!(wm.effective_gaps(ws).left, 0);
    wm.run_command("gaps inner all set 3");
    assert_eq!(wm.effective_gaps(ws).inner, 3);
    wm.run_command("gaps inner current toggle 8");
    assert_eq!(wm.effective_gaps(ws).inner, 0);
    wm.run_command("gaps inner current toggle 8");
    assert_eq!(wm.effective_gaps(ws).inner, 8);
    assert!(!wm.run_command("gaps sideways current set 1")[0].success);
}
```

- [ ] **Step 2–4:** run (FAIL), implement, run `cargo test --workspace` (PASS).
- [ ] **Step 5: Commit** `feat(gaps): runtime gaps command`.

---

### Task 6: Criteria refactor and new criteria

**Files:** Modify `crates/core/src/criteria.rs`, `crates/core/src/wm.rs`, `crates/core/src/tree.rs` (WindowData urgency sequence)

**Interfaces:**
- Produces:

```rust
pub enum UrgentPick { Latest, Oldest }
pub struct Criteria {
    pub class: Option<Regex>, pub instance: Option<Regex>, pub title: Option<Regex>,   // v1
    pub con_mark: Option<Regex>, pub con_id: Option<u32>, pub id: Option<isize>,
    pub workspace: Option<Regex>, pub floating: bool, pub tiling: bool,
    pub urgent: Option<UrgentPick>,
    pub focused_class: bool, pub focused_instance: bool, pub focused_title: bool, pub focused_workspace: bool, pub focused_con: bool, // "__focused__" values
}
impl Criteria {
    pub fn matches(&self, class: &str, instance: &str, title: &str) -> bool;   // v1, kept for assign (class/instance/title only)
    pub fn matches_node(&self, wm: &Wm, node: NodeId) -> bool;               // all keys except `urgent`
}
// WindowData gains: pub urgent_since: u64   (Wm.urgent_counter increments each time a window becomes urgent)
pub fn select_matches(wm: &Wm, c: &Criteria) -> Vec<NodeId>   // in criteria.rs: matches_node over all windows, then `urgent` keeps only urgent windows and picks the one with max (Latest) / min (Oldest) urgent_since
```

- Values: `con_id=N` = NodeId number; `id=N` = HWND as decimal; `workspace="re"`; `floating` and `tiling` have no value; `urgent=latest|oldest`; `__focused__` as value of `class`/`instance`/`title`/`workspace`/`con_id` means "equal to the focused window's / workspace's value". Keys are in i3's spelling; unknown keys stay an error (v1 test `con_mark=x` is an error must be updated: it is now valid).
- `run_command` and `for_window` use `select_matches` / `matches_node`. `assign` keeps `matches` (no node yet). `no_focus` uses `matches_node` after the node exists.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn new_criteria_keys() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.run_command("[id=2] floating enable");
    let n1 = wm.tree.window_node(WindowId(1)).unwrap();
    let n2 = wm.tree.window_node(WindowId(2)).unwrap();
    let c = parse_criteria("floating").unwrap();
    assert!(c.matches_node(&wm, n2) && !c.matches_node(&wm, n1));
    let c = parse_criteria("tiling").unwrap();
    assert!(c.matches_node(&wm, n1));
    let c = parse_criteria(&format!("con_id={}", n1.0)).unwrap();
    assert!(c.matches_node(&wm, n1) && !c.matches_node(&wm, n2));
    let c = parse_criteria("workspace=\"^1$\"").unwrap();
    assert!(c.matches_node(&wm, n1));
    let c = parse_criteria("class=__focused__").unwrap();
    assert!(c.matches_node(&wm, wm.focused()));
}
#[test]
fn urgent_latest_and_oldest() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.manage(win(3));
    wm.run_command("workspace 2");
    wm.on_focus_changed(WindowId(2)); // becomes urgent (hidden workspace)
    wm.on_focus_changed(WindowId(1));
    let latest = select_matches(&wm, &parse_criteria("urgent=latest").unwrap());
    assert_eq!(latest, vec![wm.tree.window_node(WindowId(1)).unwrap()]);
    let oldest = select_matches(&wm, &parse_criteria("urgent=oldest").unwrap());
    assert_eq!(oldest, vec![wm.tree.window_node(WindowId(2)).unwrap()]);
}
```

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS.
- [ ] **Step 5: Commit** `feat(criteria): con_id, id, workspace, floating, tiling, urgent, __focused__`.

---

### Task 7: Marks

**Files:** Modify `crates/core/src/command.rs`, `crates/core/src/wm.rs` (+ new `crates/core/src/ops/marks.rs`), `crates/core/src/criteria.rs`, `crates/core/src/ipc.rs`, `crates/core/src/tree.rs` (swap helper if needed)

**Interfaces:**
- Consumes: `Node.marks` (Task 4), `Criteria.con_mark` (Task 6), `Tree::swap(a, b)` (v1).
- Produces:

```rust
Command::Mark { add: bool, toggle: bool, name: String }   // "mark [--add|--replace] [--toggle] NAME"; replace is default
Command::Unmark(Option<String>)
Command::MoveToMark(String)                                // "move [container|window] to mark NAME"
Command::Swap(SwapTarget)                                  // "swap container with mark NAME | con_id N | id N"
pub enum SwapTarget { Mark(String), ConId(u32), Id(isize) }
pub fn marks_json(wm: &Wm) -> Value   // in ipc.rs: sorted array of all marks
IpcEvent::Window { change: "mark", .. }
```

- Rules (i3): a mark is unique in the tree — setting it on one node removes it from any other. `--replace` (default) replaces all marks of the target with NAME; `--add` appends; `--toggle` removes NAME if the target has it, else sets it (with add/replace semantics). `unmark` without name clears all marks everywhere; with name removes that mark. `move to mark`: moves the target container next to (after) the marked container, in the marked container's parent (floating if the marked one is floating), possibly on another workspace; error `"No container matched the mark"` if absent. `swap`: swaps positions of target and the other container (both tiled or both floating; error otherwise), focus stays on the same window. `con_mark` criterion matches any mark. `GET_MARKS` returns `marks_json`. Tree JSON nodes get `"marks": [...]`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn marks_are_unique_and_replace_add_toggle() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.run_command("[id=1] mark a");
    wm.run_command("[id=2] mark a");
    let n1 = wm.tree.window_node(WindowId(1)).unwrap();
    let n2 = wm.tree.window_node(WindowId(2)).unwrap();
    assert!(wm.tree.get(n1).marks.is_empty());
    assert_eq!(wm.tree.get(n2).marks, vec!["a"]);
    wm.run_command("[id=2] mark --add b");
    assert_eq!(wm.tree.get(n2).marks, vec!["a", "b"]);
    wm.run_command("[id=2] mark --add --toggle b");
    assert_eq!(wm.tree.get(n2).marks, vec!["a"]);
    wm.run_command("[id=2] mark c");
    assert_eq!(wm.tree.get(n2).marks, vec!["c"]);
    assert_eq!(marks_json(&wm), serde_json::json!(["c"]));
    wm.run_command("unmark");
    assert_eq!(marks_json(&wm), serde_json::json!([]));
}
#[test]
fn con_mark_criteria_move_to_mark_and_swap() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.manage(win(3));
    wm.run_command("[id=1] mark target");
    assert!(wm.run_command("[con_mark=\"^targ\"] kill")[0].success);
    wm.run_command("[id=3] move container to mark target");
    assert_eq!(ws_desc(&wm), "H[w1 w3 w2]");
    wm.run_command("[id=3] swap container with mark target");
    assert_eq!(ws_desc(&wm), "H[w3 w1 w2]");
    assert!(!wm.run_command("move container to mark nope")[0].success);
    check_invariants(&wm);
}
#[test]
fn mark_emits_window_mark_event() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.take_effects();
    wm.run_command("mark x");
    assert!(wm.take_effects().iter().any(|e| matches!(e, Effect::Ipc(IpcEvent::Window { change, .. }) if change == "mark")));
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat: marks, con_mark, move to mark, swap`.

---

### Task 8: Scratchpad

**Files:** Modify `crates/core/src/wm.rs`, `crates/core/src/ops/workspace.rs`, `crates/core/src/arrange.rs`, `crates/core/src/ipc.rs`, `crates/core/src/command.rs`, `crates/core/src/bar_model.rs` (exclude), new `crates/core/src/ops/scratchpad.rs`

**Interfaces:**
- Produces:

```rust
pub const SCRATCH_OUTPUT: &str = "__i3";
pub const SCRATCH_WORKSPACE: &str = "__i3_scratch";
Command::MoveScratchpad          // "move scratchpad" (also "move container to scratchpad"/"move window to scratchpad")
Command::ScratchpadShow          // "scratchpad show"
pub fn scratch_workspace(&self) -> NodeId     // Wm: created lazily in Wm::new as Output "__i3" (rect 0,0,0,0, scale 1) holding Workspace "__i3_scratch" (num None)
// Node gains: pub scratchpad: ScratchState  enum ScratchState { None, Fresh, Changed } ("none" | "fresh" | "changed" in tree JSON as scratchpad_state)
```

- The `__i3` output is a child of root but: `Wm::outputs()` skips it; `set_outputs` keeps it untouched; `arrange` puts every window under it into `hidden` and arranges nothing there; `workspaces_json`, `outputs_json`, bars (`bar_model::workspace_buttons` uses outputs), `workspace next/prev`, `focus output`, `move workspace to output` never see it. `tree_json` does include it (i3 shape).
- `move scratchpad`: the target window becomes floating (keep its floating_rect if any, else its tiled size centered) and moves to `__i3_scratch`; `scratchpad` = Fresh; focus goes to the next window as for a close.
- `scratchpad show` without criteria (i3): if the focused window is a scratchpad window (scratchpad != None) and is on a visible workspace → send it back to `__i3_scratch`. Else if a scratchpad window is visible on the focused workspace but not focused → focus it. Else take the scratchpad window that was hidden most recently (track an order Vec in Wm), move it floating to the focused workspace, centered on the output if Fresh (then mark Changed), keep its last rect if Changed, focus it. Error `"No scratchpad windows"` if none exist anywhere.
- With criteria: for each matched window that is a scratchpad window: if it's visible on the focused workspace → hide it to scratch; else show it on the focused workspace as above. Matched windows that aren't scratchpad windows: error `"Matched window is not a scratchpad window"` (i3 message).
- Moving a scratchpad window out with `floating disable` or `move to workspace` resets `scratchpad` to None (it's no longer a scratchpad window).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn scratchpad_round_trip() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.run_command("[id=2] move scratchpad");
    assert_eq!(ws_desc(&wm), "H[w1]");
    let n2 = wm.tree.window_node(WindowId(2)).unwrap();
    assert_eq!(wm.tree.workspace_of(n2), Some(wm.scratch_workspace()));
    wm.run_command("scratchpad show");
    assert_eq!(wm.tree.workspace_of(n2), Some(wm.focused_workspace()));
    assert!(wm.tree.is_floating(n2));
    assert_eq!(wm.focused_window(), Some(WindowId(2)));
    wm.run_command("scratchpad show");
    assert_eq!(wm.tree.workspace_of(n2), Some(wm.scratch_workspace()));
    assert!(!test_wm().run_command("scratchpad show")[0].success);
    check_invariants(&wm);
}
#[test]
fn scratchpad_windows_are_in_arrangement_hidden() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("move scratchpad");
    let a = wm.arrange();
    assert!(a.hidden.contains(&WindowId(1)));
    assert!(a.windows.iter().all(|p| p.id != WindowId(1)));
}
#[test]
fn scratch_output_is_invisible_everywhere() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("move scratchpad");
    assert_eq!(wm.outputs().len(), 1);
    let outs = outputs_json(&wm);
    assert!(outs.as_array().unwrap().iter().all(|o| o["name"] != "__i3"));
    let wss = workspaces_json(&wm);
    assert!(wss.as_array().unwrap().iter().all(|w| w["name"] != "__i3_scratch"));
    wm.run_command("workspace next");
    assert_ne!(wm.workspace_name(wm.focused_workspace()), "__i3_scratch");
    assert!(tree_json(&wm).to_string().contains("__i3_scratch"));
    wm.set_outputs(vec![out("O1", 0, 800)]);
    assert!(wm.tree.contains(wm.scratch_workspace()));
}
#[test]
fn scratchpad_show_with_criteria_toggles_matching() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.run_command("[id=2] move scratchpad");
    wm.run_command("[id=2] scratchpad show");
    assert_eq!(wm.focused_window(), Some(WindowId(2)));
    wm.run_command("[id=2] scratchpad show");
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
    assert!(!wm.run_command("[id=1] scratchpad show")[0].success);
}
```

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS. Run the v1 live smoke test is not needed; the hide path is v1's.
- [ ] **Step 5: Commit** `feat: scratchpad`.

---

### Task 9: Sticky

**Files:** Modify `crates/core/src/command.rs`, `crates/core/src/ops/workspace.rs` (`show_workspace`), `crates/core/src/tree.rs` (Node field), `crates/core/src/ipc.rs`

**Interfaces:**
- Produces: `Node.sticky: bool`; `Command::Sticky(Toggle)`; tree JSON `"sticky": bool`.
- Semantics: only floating windows can be sticky in effect (tiled windows store the flag but nothing happens, as in i3). In `show_workspace(ws)`, before hiding the old visible workspace of that output, every sticky floating window on it is re-attached (floating, same rect) to `ws`. Focus: if the focused window was sticky it stays focused.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn sticky_floating_follows_workspace_switch() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.run_command("[id=2] floating enable, sticky enable");
    wm.run_command("workspace 3");
    let n2 = wm.tree.window_node(WindowId(2)).unwrap();
    assert_eq!(wm.workspace_name(wm.tree.workspace_of(n2).unwrap()), "3");
    assert!(wm.arrange().windows.iter().any(|p| p.id == WindowId(2)));
    wm.run_command("[id=2] sticky toggle");
    wm.run_command("workspace 1");
    assert_eq!(wm.workspace_name(wm.tree.workspace_of(n2).unwrap()), "3");
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat: sticky`.

---

### Task 10: Workspace rename and auto back-and-forth

**Files:** Modify `crates/core/src/command.rs`, `crates/core/src/ops/workspace.rs`, `crates/core/src/wm.rs`, `crates/core/src/ipc.rs`

**Interfaces:**
- Produces: `Command::RenameWorkspace { old: Option<String>, new: String }` (`rename workspace [OLD] to NEW`, also `rename workspace to NEW`); `Command::Workspace` becomes `Command::Workspace { target: WorkspaceTarget, no_auto_back_and_forth: bool }` (`workspace --no-auto-back-and-forth NAME`); `IpcEvent::Workspace { change: "rename", current: Some(new), old: None }` (event JSON `current` is the renamed workspace).
- Rename: error `"Workspace OLD not found"` / `"New workspace name \"NEW\" already in use"`; `num` is recomputed from the new name; `prev_workspace` is updated if it held OLD; the workspace is re-sorted on its output (`attach_workspace_sorted`); if `workspace_outputs` assigns NEW to another output, it moves there (i3 behavior); emits `rename`.
- Auto back-and-forth: with `workspace_auto_back_and_forth yes`, `workspace NAME`/`workspace number N` targeting the already-focused workspace acts as `workspace back_and_forth`, unless `--no-auto-back-and-forth`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn rename_workspace() {
    let mut wm = test_wm();
    wm.manage(win(1));
    assert!(wm.run_command("rename workspace to \"5: mail\"")[0].success);
    let ws = wm.focused_workspace();
    assert_eq!(wm.workspace_name(ws), "5: mail");
    assert!(matches!(wm.tree.get(ws).kind, NodeKind::Workspace { num: Some(5), .. }));
    wm.run_command("workspace 2");
    assert!(wm.run_command("rename workspace \"5: mail\" to 7")[0].success);
    assert!(wm.find_workspace("7").is_some());
}
#[test]
fn rename_to_existing_name_fails() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("workspace 2");
    wm.manage(win(2));
    let r = wm.run_command("rename workspace to 1");
    assert!(!r[0].success);
    assert_eq!(wm.workspace_name(wm.focused_workspace()), "2");
    assert!(!wm.run_command("rename workspace nope to x")[0].success);
}
#[test]
fn auto_back_and_forth() {
    let mut cfg = Config::default();
    cfg.workspace_auto_back_and_forth = true;
    let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    wm.run_command("workspace 2");
    wm.manage(win(2));
    wm.run_command("workspace 2");
    assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
    wm.run_command("workspace --no-auto-back-and-forth 1");
    assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat(workspace): rename, auto back-and-forth`.

---

### Task 11: Focus wrapping and `focus next|prev [sibling]`

**Files:** Modify `crates/core/src/ops/focus.rs`, `crates/core/src/command.rs`

**Interfaces:**
- Produces: `Command::FocusNext { forward: bool, sibling: bool }` (`focus next`, `focus prev`, `focus next sibling`, `focus prev sibling`).
- `focus_direction` algorithm with `config.focus_wrapping` (v1 behavior = `No`):

```
walk up from `from` as v1 does, looking for an ancestor P with P.layout.orientation() == dir.orientation()
and a sibling of the walked-up child in `dir`:
  - found: focus it (v1).
  - Force: at the FIRST ancestor P with matching orientation and >= 2 children, if there is no sibling in dir,
    wrap inside P: focus the focused tiling descendant of P's first child (dir forward) or last child (backward). Stop.
reached the workspace without a sibling:
  - No:        focus the adjacent output if any (v1), else nothing.
  - Yes:       adjacent output if any; else wrap (below).
  - Workspace: never an adjacent output; wrap (below).
  - wrap = take the OUTERMOST ancestor (closest to the workspace, including the workspace) with matching
    orientation and >= 2 children that contains `from`, and focus the focused tiling descendant of its
    first (forward) / last (backward) child. No such ancestor: nothing.
```

- `focus next|prev`: move focus to the next/previous child in the focused container's parent, wrapping, going down to that child's focused tiling descendant. `sibling` variant focuses the sibling container itself (no descent). Works on tiled; on floating cycles floating windows like v1 floating `focus left/right`.

- [ ] **Step 1: Failing tests**

```rust
fn wm_with(fw: &str) -> Wm {
    let cfg = crate::config::parse_config(&format!("focus_wrapping {fw}\n"));
    let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
    wm.manage(win(1)); wm.manage(win(2)); wm.manage(win(3));
    wm
}
#[test]
fn focus_wrapping_modes() {
    let mut wm = wm_with("yes");
    wm.run_command("focus right");
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
    let mut wm = wm_with("no");
    wm.run_command("focus right");
    assert_eq!(wm.focused_window(), Some(WindowId(3)));
    let mut wm = wm_with("workspace");
    wm.run_command("focus right");
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
}
#[test]
fn focus_wrapping_force_stays_in_parent() {
    let mut wm = wm_with("force");
    // w3 into a vertical split with a new w4: H[w1 w2 V[w3 w4]]
    wm.run_command("split v");
    wm.manage(win(4));
    wm.run_command("focus down");
    assert_eq!(wm.focused_window(), Some(WindowId(3)));
}
#[test]
fn focus_wrapping_yes_prefers_other_output() {
    let cfg = crate::config::parse_config("focus_wrapping yes\n");
    let mut wm = Wm::new(cfg, vec![out("A", 0, 1000), out("B", 1000, 1000)]);
    wm.manage(win(1));
    wm.run_command("focus right");
    assert_eq!(wm.focused_output(), wm.outputs()[1]);
}
#[test]
fn focus_next_prev() {
    let mut wm = wm_with("yes");
    wm.run_command("focus next");
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
    wm.run_command("focus prev");
    assert_eq!(wm.focused_window(), Some(WindowId(3)));
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS (v1 focus tests that assumed no wrapping use `Config::default()`, now `Yes`: update them to set `focus_wrapping no` where they test the edge, or adjust expectations — never delete).
- [ ] **Step 5: Commit** `feat(focus): focus_wrapping, focus next/prev [sibling]`.

---

### Task 12: Window activation, popups during fullscreen, no_focus

**Files:** Modify `crates/core/src/wm.rs` (`manage`, `on_focus_changed`)

**Interfaces:**
- Consumes: `config.focus_on_window_activation`, `config.popup_during_fullscreen`, `config.no_focus` (Task 2), `matches_node` (Task 6).
- `on_focus_changed(id)` for a window on a **visible** workspace: unchanged (user clicks always work). For a window on a **hidden** workspace: `Smart`/`Urgent` → mark urgent, push focus back (v1); `Focus` → switch to its workspace (`show_workspace`) and focus it; `None` → push focus back, do not mark urgent.
- `manage(info)` when the target workspace is the focused one and it has a fullscreen window F:
  - `Smart`: if the new window is floating and `class` equals F's class → it shows on top (v1 already keeps floating above fullscreen) and gets focus; otherwise the new window is managed without taking focus (`set_focused_within` its workspace) and F keeps focus.
  - `Ignore`: the new window never takes focus.
  - `LeaveFullscreen`: F's fullscreen is disabled first, then the window is managed normally.
- `no_focus` match: the new window does not take focus, unless it is the only window on its workspace (i3 rule).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn activation_focus_switches_workspace() {
    let mut wm = Wm::new(crate::config::parse_config("focus_on_window_activation focus\n"), vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    wm.run_command("workspace 2");
    wm.manage(win(2));
    wm.on_focus_changed(WindowId(1));
    assert_eq!(wm.workspace_name(wm.focused_workspace()), "1");
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
}
#[test]
fn activation_none_still_allows_visible_focus() {
    let mut wm = Wm::new(crate::config::parse_config("focus_on_window_activation none\n"), vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    wm.manage(win(2));
    wm.on_focus_changed(WindowId(1));
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
    wm.run_command("workspace 2");
    wm.manage(win(3));
    wm.on_focus_changed(WindowId(1));
    assert_eq!(wm.focused_window(), Some(WindowId(3)));
    let n1 = wm.tree.window_node(WindowId(1)).unwrap();
    assert!(matches!(&wm.tree.get(n1).kind, NodeKind::Window(d) if !d.urgent));
}
#[test]
fn popup_during_fullscreen_smart() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("fullscreen enable");
    wm.manage(win(2)); // different class, tiled
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
    let mut dlg = win(3);
    dlg.class = "c1".into();
    dlg.wants_float = true;
    wm.manage(dlg);
    assert_eq!(wm.focused_window(), Some(WindowId(3)));
}
#[test]
fn popup_leave_fullscreen() {
    let mut wm = Wm::new(crate::config::parse_config("popup_during_fullscreen leave_fullscreen\n"), vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    wm.run_command("fullscreen enable");
    wm.manage(win(2));
    assert!(wm.arrange().fullscreen_outputs.is_empty());
    assert_eq!(wm.focused_window(), Some(WindowId(2)));
}
#[test]
fn no_focus_criteria() {
    let mut wm = Wm::new(crate::config::parse_config("no_focus [class=\"^c2$\"]\n"), vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    wm.manage(win(2)); // class c2: matched, must not take focus
    assert_eq!(wm.focused_window(), Some(WindowId(1)));
    wm.run_command("workspace 4");
    let mut lone = win(4);
    lone.class = "c2".into();
    wm.manage(lone); // matched, but alone on its workspace: takes focus
    assert_eq!(wm.focused_window(), Some(WindowId(4)));
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat: focus_on_window_activation, popup_during_fullscreen, no_focus`.

---

### Task 13: Floating size limits, move position, resize set

**Files:** Modify `crates/core/src/command.rs`, `crates/core/src/ops/floating.rs`, `crates/core/src/ops/movement.rs`, `crates/core/src/ops/layout_ops.rs`, `crates/core/src/wm.rs`, `crates/i3win/src/app.rs` (pointer)

**Interfaces:**
- Produces:

```rust
pub enum Len { Px(i32), Ppt(i32) }
Command::MovePosition { x: Len, y: Len }        // "move [window|container] [to] position X [px|ppt] Y [px|ppt]" (unit default px)
Command::MoveCenter { absolute: bool }          // "move [absolute] position center"
Command::MoveToPointer                          // "move position mouse" / "move position cursor" / "move position pointer"
Command::ResizeSet { w: Option<Len>, h: Option<Len> }  // "resize set [width] W [px|ppt] [[height] H [px|ppt]]"; 0 or missing = unchanged
pub pointer: Option<(i32, i32)>                 // Wm field; App sets it from GetCursorPos before every run_command/binding
pub fn clamp_floating_size(cfg: &Config, w: i32, h: i32) -> (i32, i32)   // floating_minimum_size / maximum (0 = no limit; min (0,0) = only MIN_FLOAT_SIZE)
```

- `move position` applies to floating windows (ppt relative to the output). On a tiled window it does nothing and still reports success (i3 behavior).
- `move absolute position center` centers on the whole output bounding box of all outputs; without `absolute`, on the window's output. `move position mouse` centers the window on `pointer`, clamped to its output.
- `resize set` on floating: sets the frame size (ppt of the output), clamped by `clamp_floating_size`; on tiled: sets the window's percent in the nearest `splith` (width) / `splitv` (height) ancestor so its size becomes W/H (ppt = percent directly), siblings shrink/grow proportionally, each child ≥ 5%.
- Every floating rect change (set_floating, move, resize, `on_floating_moved`) passes through `clamp_floating_size`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn move_position_and_center() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("floating enable");
    wm.run_command("move position 10 px 20 px");
    let n = wm.tree.window_node(WindowId(1)).unwrap();
    let fr = |wm: &Wm| match &wm.tree.get(n).kind { NodeKind::Window(d) => d.floating_rect.unwrap(), _ => unreachable!() };
    assert_eq!((fr(&wm).x, fr(&wm).y), (10, 20));
    wm.run_command("move position 50 ppt 0 ppt");
    assert_eq!(fr(&wm).x, 500);
    wm.run_command("move position center");
    let r = fr(&wm);
    assert_eq!(r.x + r.w / 2, 500);
    wm.pointer = Some((600, 250));
    wm.run_command("move position mouse");
    let r = fr(&wm);
    assert_eq!((r.x + r.w / 2, r.y + r.h / 2), (600, 250));
    wm.pointer = Some((990, 490)); // near the corner: clamped inside the output
    wm.run_command("move position mouse");
    let r = fr(&wm);
    assert_eq!((r.right(), r.bottom()), (1000, 500));
}
#[test]
fn resize_set_floating_and_tiled() {
    let mut wm = Wm::new(crate::config::parse_config("floating_maximum_size 600 x 400\n"), vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    wm.manage(win(2));
    assert!(wm.run_command("resize set 30 ppt")[0].success);
    let n2 = wm.tree.window_node(WindowId(2)).unwrap();
    assert!((wm.tree.get(n2).percent - 0.30).abs() < 1e-9);
    wm.run_command("floating enable");
    wm.run_command("resize set 900 px 900 px");
    match &wm.tree.get(n2).kind { NodeKind::Window(d) => { let r = d.floating_rect.unwrap(); assert_eq!((r.w, r.h), (600, 400)); } _ => unreachable!() }
}
#[test]
fn floating_minimum_size_applies() {
    let mut wm = Wm::new(crate::config::parse_config("floating_minimum_size 200 x 150\n"), vec![out("O1", 0, 1000)]);
    let mut w = win(1);
    w.rect = Rect::new(0, 0, 60, 60);
    w.wants_float = true;
    wm.manage(w);
    let n = wm.tree.window_node(WindowId(1)).unwrap();
    match &wm.tree.get(n).kind { NodeKind::Window(d) => { let r = d.floating_rect.unwrap(); assert_eq!((r.w, r.h), (200, 150)); } _ => unreachable!() }
}
```

- [ ] **Step 2–4:** FAIL → implement (App sets `wm.pointer` via `GetCursorPos` in `on_binding` and `on_main_msg` RunCommand/IPC paths) → PASS.
- [ ] **Step 5: Commit** `feat: move position, resize set, floating size limits`.

---

### Task 14: `move container to output`, `layout toggle LIST`

**Files:** Modify `crates/core/src/command.rs`, `crates/core/src/ops/workspace.rs`, `crates/core/src/ops/layout_ops.rs`

**Interfaces:**
- Produces: `Command::MoveToOutput(OutputTarget)` (`move [container|window] to output left|right|up|down|NAME`); `LayoutArg::ToggleList(Vec<LayoutChoice>)` with `enum LayoutChoice { Set(Layout), Split }` (`layout toggle tabbed stacking split`; `split` stands for splith/splitv and keeps the last split orientation).
- `move to output`: moves the target to the visible workspace of that output (like `move to workspace`, focus stays on the current workspace).
- `layout toggle LIST`: if the current layout is in the list (splith/splitv both match `Split`), switch to the next entry (wrapping); else switch to the first. `layout toggle split` and `toggle all` keep v1 meaning.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn move_container_to_output() {
    let mut wm = Wm::new(Config::default(), vec![out("A", 0, 1000), out("B", 1000, 1000)]);
    wm.manage(win(1));
    wm.manage(win(2));
    assert!(wm.run_command("move container to output right")[0].success);
    let b_ws = wm.visible_workspace(wm.outputs()[1]);
    assert_eq!(wm.tree.describe(b_ws), "H[w2]");
    assert_eq!(wm.focused_output(), wm.outputs()[0]);
    assert!(wm.run_command("move container to output B")[0].success);
    assert!(!wm.run_command("move container to output nope")[0].success);
}
#[test]
fn layout_toggle_list() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.run_command("layout toggle tabbed stacking split");
    assert_eq!(ws_desc(&wm), "T[w1 w2]");
    wm.run_command("layout toggle tabbed stacking split");
    assert_eq!(ws_desc(&wm), "S[w1 w2]");
    wm.run_command("layout toggle tabbed stacking split");
    assert_eq!(ws_desc(&wm), "H[w1 w2]");
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Commit** `feat: move container to output, layout toggle list`.

---

### Task 15: `--release` bindings and `bindcode`

**Files:**
- Modify: `crates/core/src/keys.rs`, `crates/core/src/config.rs`, `crates/core/src/wm.rs`
- Create: `crates/core/src/keycodes.rs` (+ `pub mod keycodes;`)
- Modify: `crates/i3win/src/win/keyboard.rs`, `crates/i3win/src/mainloop.rs`, `crates/i3win/src/app.rs`

**Interfaces:**
- Produces:

```rust
// keys.rs
pub const SCANCODE_FLAG: u16 = 0x8000;      // KeyCombo.vk with this bit = a scan code: bits 0-7 code, bit 8 (0x100) extended
pub fn scancode_combo(mods: Modifiers, scancode: u8, extended: bool) -> KeyCombo
// keycodes.rs
pub fn x11_keycode_to_scancode(code: u16) -> Option<(u8, bool)>   // (scancode set 1, extended)
// config.rs
pub struct Binding { pub combo: KeyCombo, pub command: String, pub release: bool }
// wm.rs
pub fn binding_pressed(&mut self, combo: KeyCombo, release: bool) -> bool   // finds the binding with that combo AND release flag
pub fn active_combos(&self) -> Vec<(KeyCombo, bool)>                        // (combo, release)
// keyboard.rs
pub fn set_combos(combos: &[(KeyCombo, bool)])
KeyAction::Fire { combo: KeyCombo, mask_menu: bool, release: bool }
```

- `bindsym --release COMBO cmd` and `bindcode [--release] [MODS+]CODE cmd` (e.g. `bindcode $mod+38 nop`). `x11_keycode_to_scancode`: X11 keycode = evdev code + 8; evdev codes 1–88 equal PC set-1 scan codes (non-extended). Extended keys use this table (X11 keycode → set-1 scancode, extended=true): 104 KP_Enter→0x1C, 105 Control_R→0x1D, 106 KP_Divide→0x35, 107 Print→0x37, 108 Alt_R→0x38, 110 Home→0x47, 111 Up→0x48, 112 Prior→0x49, 113 Left→0x4B, 114 Right→0x4D, 115 End→0x4F, 116 Down→0x50, 117 Next→0x51, 118 Insert→0x52, 119 Delete→0x53, 133 Super_L→0x5B, 134 Super_R→0x5C, 135 Menu→0x5D. Non-extended extras: 95 F11→0x57, 96 F12→0x58, 127 Pause→0x45 (special: Pause is 0x45 with no extended flag in KBDLLHOOKSTRUCT). Unknown code → config warning `bindcode: unknown keycode N`, line skipped.
- Matcher: stores `HashMap<KeyCombo, (bool /*press*/, bool /*release*/)>`. On key down (non-own, non-modifier) build `KeyCombo{mods, vk}` and `scancode_combo(mods, info.scanCode as u8, info.flags & LLKHF_EXTENDED != 0)`; if either is bound: swallow the press; fire immediately if it has a press binding; remember `(vk → combo)` for release. On that key's up: swallow it, and fire the release binding if any (with the mods held at press time). Mainloop passes `release` into `App::on_binding(combo, release)` → `wm.binding_pressed(combo, release)`.

- [ ] **Step 1: Failing tests**

```rust
// keycodes.rs
#[test]
fn keycode_table() {
    assert_eq!(x11_keycode_to_scancode(38), Some((0x1E, false))); // a
    assert_eq!(x11_keycode_to_scancode(36), Some((0x1C, false))); // Return
    assert_eq!(x11_keycode_to_scancode(113), Some((0x4B, true))); // Left
    assert_eq!(x11_keycode_to_scancode(105), Some((0x1D, true))); // Control_R
    assert_eq!(x11_keycode_to_scancode(255), None);
}
// config.rs
#[test]
fn bindcode_and_release_parse() {
    let c = parse_config("set $mod Mod1\nbindcode $mod+38 nop a\nbindsym --release $mod+x nop b\nbindcode 300 nop\n");
    let b = &c.bindings["default"];
    assert_eq!(b[0].combo, scancode_combo(Modifiers::ALT, 0x1E, false));
    assert!(!b[0].release);
    assert!(b[1].release);
    assert_eq!(c.warnings.len(), 1);
}
// wm.rs
#[test]
fn release_binding_only_fires_on_release() {
    let mut wm = Wm::new(crate::config::parse_config("bindsym --release Mod1+x mode resize\nmode \"resize\" {\nbindsym Escape mode default\n}\n"), vec![out("O1", 0, 1000)]);
    let combo = parse_combo("Mod1+x").unwrap();
    assert!(!wm.binding_pressed(combo, false));
    assert!(wm.binding_pressed(combo, true));
    assert_eq!(wm.mode, "resize");
    assert_eq!(wm.active_combos(), vec![(parse_combo("Escape").unwrap(), false)]);
}
// keyboard.rs (KeyMatcher)
#[test]
fn release_combo_fires_on_key_up() {
    let mut k = KeyMatcher::new();
    k.set_combos(&[(parse_combo("Mod1+x").unwrap(), true)]);
    k.on_key(0xA4, true, false, 0, false);                      // Alt down
    assert_eq!(k.on_key(0x58, true, false, 0x2D, false), KeyAction::Swallow);
    assert!(matches!(k.on_key(0x58, false, false, 0x2D, false), KeyAction::Fire { release: true, .. }));
}
#[test]
fn scancode_combo_matches() {
    let mut k = KeyMatcher::new();
    k.set_combos(&[(scancode_combo(Modifiers::default(), 0x1E, false), false)]);
    assert!(matches!(k.on_key(0x41, true, false, 0x1E, false), KeyAction::Fire { release: false, .. }));
}
```

(`on_key` gains `scancode: u8, extended: bool` parameters; update its existing tests to pass `0, false`.)

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS.
- [ ] **Step 5: Commit** `feat(bindings): --release and bindcode`.

---

### Task 16: IPC additions, live checks, docs

**Files:** Modify `crates/core/src/ipc.rs`, `crates/core/src/wm.rs` (event emission), `scripts/live-checks.ps1`, `docs/manual-checks.md`

**Interfaces:**
- Consumes: everything above.
- Tree JSON nodes gain: `"marks"` (Task 7 may already have it), `"sticky"`, `"scratchpad_state"` (`"none"|"fresh"|"changed"`), `"title_format"` (string or null), `"window_icon_padding"` (`-1` when off, else padding). Window events emitted where missing: `move` (move direction, move to workspace/output/mark/scratchpad, swap), `floating` (floating enable/disable/toggle), `urgent` (a window becomes or stops being urgent), `title` exists; workspace `rename` exists from Task 10.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn tree_json_v2_fields() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.run_command("mark m, sticky enable, title_format x, title_window_icon padding 3px");
    let t = tree_json(&wm).to_string();
    for s in ["\"marks\":[\"m\"]", "\"sticky\":true", "\"scratchpad_state\":\"none\"", "\"title_format\":\"x\"", "\"window_icon_padding\":3"] {
        assert!(t.contains(s), "missing {s} in {t}");
    }
}
#[test]
fn window_move_floating_urgent_events() {
    let mut wm = test_wm();
    wm.manage(win(1));
    wm.manage(win(2));
    wm.take_effects();
    wm.run_command("move left");
    wm.run_command("floating toggle");
    wm.run_command("workspace 2");
    wm.on_focus_changed(WindowId(1));
    let changes: Vec<String> = wm.take_effects().into_iter().filter_map(|e| match e {
        Effect::Ipc(IpcEvent::Window { change, .. }) => Some(change), _ => None }).collect();
    for c in ["move", "floating", "urgent"] {
        assert!(changes.iter().any(|x| x == c), "missing {c}: {changes:?}");
    }
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Live checks.** Append to `scripts/live-checks.ps1`, before cleanup, checks using `i3win-msg`: `mark x` then `-t get_marks` contains `"x"`; `move scratchpad` hides the focused Notepad (`IsWindowVisible` false) and `scratchpad show` shows it; `rename workspace to 9` then `get_workspaces` contains `"9"`; `gaps inner current set 20` changes the Notepads' rects. Add "Plan 5" items to `docs/manual-checks.md`: title icons visible with `for_window [class=".*"] title_window_icon on`; `border pixel 4` draws 4 px; `hide_edge_borders both` hides outer edges; `bindcode $mod+38 exec notepad` opens Notepad with Alt+A on any keyboard layout; `--release` binding fires on key up. Do not run the live script (it rearranges the desktop); the controller runs it.
- [ ] **Step 6: Commit** `feat(ipc): v2 tree fields and window events; live checks`.
