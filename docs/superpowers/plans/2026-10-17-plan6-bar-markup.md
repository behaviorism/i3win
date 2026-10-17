# Plan 6: Bar and Markup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render every i3bar block field that matters for looks (including Pango markup), support all of i3's bar options except the tray, multiple `bar {}` blocks, hide mode, bar button bindings and `bar` commands.

**Architecture:** A Pango-subset markup parser and the bar layout stay pure (`i3win-core::markup`, `i3win::bar_layout`, tested without GDI). GDI draws text runs. `Config.bar: Option<BarConfig>` becomes `Config.bars: Vec<BarConfig>`; the platform keeps one set of bar windows and one status process per bar id.

**Tech Stack:** Rust 2021, `windows` 0.58 GDI, `serde_json`.

**Spec:** `docs/superpowers/specs/2026-10-17-i3win-v2-design.md` (sections "Bar", "Markup", "Bar config additions", "Mouse → Bar and title bar mouse", "IPC additions").

## Global Constraints

- Same as Plan 5's Global Constraints (pure core, `windows` 0.58, i3-shaped IPC, no crashes on bad config, no blocking cross-process calls, `cargo test/clippy -D warnings/fmt --check` green before every commit, commit trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`).
- No tray: `tray_output` and `tray_padding` are accepted and ignored without a warning.
- Bar defaults (i3): `position bottom`, `mode dock`, `hidden_state hide`, `modifier Mod4`, `workspace_buttons yes`, `binding_mode_indicator yes`, `strip_workspace_numbers no`, `strip_workspace_name no`, no `separator_symbol` (a 1 px line).
- Default bar ids: `bar-0`, `bar-1`, ... in file order, unless `id` is set.
- A status update must only redraw bars (v1 ruling); hide-mode toggling must not re-arrange windows (hide-mode bars reserve no space).

## Review Focus

1. Malformed markup (`<b>unclosed`, `<span foo=>`, stray `&`) renders as the plain text, never panics or blanks the block (Task 1 test `malformed_markup_falls_back_to_plain`).
2. A bar block with `output` naming a monitor that doesn't exist creates no windows and no crash; reconnecting the monitor creates them (Task 6 test `bar_outputs_filter`).
3. Two bars with status commands: a click on bar-1 must go to bar-1's status process only (Task 6 test `click_routes_to_owning_bar`).
4. `short_text` switching must happen for all blocks together before dropping blocks (Task 4 test `short_text_used_before_dropping`).
5. Holding the hide modifier with no hide-mode bar must not post messages from the keyboard hook on every key (Task 7 test `modifier_watch_disabled_without_hide_bars`).

---

### Task 1: Markup parser

**Files:** Create `crates/core/src/markup.rs` (+ `pub mod markup;` in `lib.rs`)

**Interfaces:**
- Produces:

```rust
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Style {
    pub bold: bool, pub weight: Option<u16>, pub italic: bool, pub underline: bool, pub strike: bool,
    pub mono: bool, pub fg: Option<Color>, pub bg: Option<Color>,
    pub scale: f32,            // 1.0 = normal; small 0.833, big 1.2, x-small 0.694, x-large 1.44, sup/sub 0.75; NNpt -> handled by `size_pt`
    pub size_pt: Option<f32>,  // absolute size from size="NNpt" or Pango size units (NN/1024 pt)
    pub family: Option<String>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Run { pub text: String, pub style: Style }
pub fn parse_markup(s: &str) -> Vec<Run>     // never fails: malformed input -> one Run with the input as plain text (entities NOT decoded)
pub fn plain_text(s: &str) -> String         // runs' text concatenated (for measuring/fallback)
pub fn escape_markup(s: &str) -> String      // & < > " ' -> entities
```

- Tags: `b`, `i`, `u`, `s`, `small`, `big`, `sup`, `sub`, `tt` (mono), `span` with attributes `foreground|fgcolor|color`, `background|bgcolor`, `weight` (`bold|normal|light|ultrabold|heavy|NNN`), `style` (`italic|oblique|normal`), `underline` (`single|double|low` = on, `none` = off), `strikethrough` (`true|false`), `size` (`xx-small|x-small|small|medium|large|x-large|xx-large|smaller|larger`, `NNpt`, `NN%`, or a bare integer in Pango units), `font_family|face`, `rise` (ignored), `font|font_desc` (ignored). Unknown tags: dropped, their text kept. Unknown attributes: ignored. Nested tags stack; closing tags must match the open stack top, otherwise the whole input is malformed. Entities `&amp; &lt; &gt; &quot; &apos; &#NN; &#xHH;`; an unknown entity or a bare `&` makes the input malformed. Colors: `#rgb`, `#rrggbb`, `#rrggbbaa`, and the names `black white red green blue yellow cyan magenta gray grey orange purple`. Adjacent runs with equal style are merged.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn plain_and_entities() {
    assert_eq!(parse_markup("a &amp; b &#65;&#x42;"), vec![Run { text: "a & b AB".into(), style: Style { scale: 1.0, ..Default::default() } }]);
}
#[test]
fn nested_tags_and_span() {
    let r = parse_markup("x<b>y<i>z</i></b><span foreground='#ff0000' size='large'>w</span>");
    assert_eq!(r.len(), 4);
    assert_eq!(r[1].text, "y"); assert!(r[1].style.bold && !r[1].style.italic);
    assert_eq!(r[2].text, "z"); assert!(r[2].style.bold && r[2].style.italic);
    assert_eq!(r[3].style.fg, Some(Color(255, 0, 0)));
    assert!((r[3].style.scale - 1.2).abs() < 1e-6);
}
#[test]
fn unknown_tags_dropped_text_kept() {
    assert_eq!(plain_text_of(&parse_markup("a<blink>b</blink>c")), "abc");
}
#[test]
fn malformed_markup_falls_back_to_plain() {
    for bad in ["<b>unclosed", "<b>x</i>", "a & b", "&bogus;", "<span foo=>x</span", "</b>"] {
        let r = parse_markup(bad);
        assert_eq!(r.len(), 1, "{bad}");
        assert_eq!(r[0].text, bad);
    }
}
#[test]
fn escape_round_trip() {
    let s = "a<b>&\"'";
    assert_eq!(plain_text(&escape_markup(s)), s);
}
fn plain_text_of(r: &[Run]) -> String { r.iter().map(|x| x.text.as_str()).collect() }
```

- [ ] **Step 2–4:** run `cargo test -p i3win-core markup` (FAIL) → implement → PASS.
- [ ] **Step 5: Commit** `feat(core): pango markup subset parser`.

---

### Task 2: i3bar block fields and click JSON

**Files:** Modify `crates/core/src/i3bar.rs`, `crates/i3win/src/status.rs`, `crates/i3win/src/mainloop.rs` (`MainMsg::StatusClick` fields), `crates/i3win/src/win/bar.rs` (fill them)

**Interfaces:**
- Produces:

```rust
// Block gains (serde names = i3bar protocol names, all optional):
pub min_width: Option<MinWidth>,        // #[serde(untagged)] enum MinWidth { Px(i32), Text(String) }
pub align: Option<String>,              // "left" | "center" | "right" (default left)
pub markup: Option<String>,             // "pango" | "none"
pub border_top: Option<i32>, pub border_right: Option<i32>, pub border_bottom: Option<i32>, pub border_left: Option<i32>,
// Click:
pub struct Click { pub name: Option<String>, pub instance: Option<String>, pub button: u8, pub modifiers: Vec<String>,
                   pub x: i32, pub y: i32, pub relative_x: i32, pub relative_y: i32,
                   pub output_x: i32, pub output_y: i32, pub width: i32, pub height: i32 }
impl ClickWriter { pub fn encode(&mut self, c: &Click) -> String }   // replaces the 5-arg encode
// MainMsg::StatusClick { bar: String, click: Click }   (bar id; Task 6 routes by it, until then ignore it)
```

- `modifiers`: names as i3 sends them: `"Shift"`, `"Control"`, `"Mod1"`, `"Mod4"` (from `GetKeyState` at click time). `x,y` = screen coords; `relative_*` = inside the block; `output_*` = relative to the bar's monitor origin; `width,height` = block rect size.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn new_block_fields_parse() {
    let mut p = StatusParser::new();
    p.feed_line(r#"{"version":1}"#); p.feed_line("[");
    let b = p.feed_line(r#"[{"full_text":"x","min_width":"100%","align":"center","markup":"pango","border_top":2},{"full_text":"y","min_width":50}]"#).unwrap();
    assert_eq!(b[0].min_width, Some(MinWidth::Text("100%".into())));
    assert_eq!(b[0].align.as_deref(), Some("center"));
    assert_eq!(b[0].border_top, Some(2));
    assert_eq!(b[1].min_width, Some(MinWidth::Px(50)));
}
#[test]
fn click_json_has_i3_fields() {
    let mut w = ClickWriter::new();
    let c = Click { name: Some("cpu".into()), instance: None, button: 1, modifiers: vec!["Shift".into()],
        x: 10, y: 20, relative_x: 3, relative_y: 4, output_x: 10, output_y: 5, width: 50, height: 18 };
    let s = w.encode(&c);
    assert!(s.starts_with("[\n"));
    let v: serde_json::Value = serde_json::from_str(s.trim_start_matches("[\n").trim()).unwrap();
    for k in ["name","instance","button","modifiers","x","y","relative_x","relative_y","output_x","output_y","width","height"] {
        assert!(v.get(k).is_some(), "missing {k}");
    }
    assert!(w.encode(&c).starts_with(','));
}
```

- [ ] **Step 2–4:** FAIL → implement (update `bar.rs::click` to compute all fields from the item rect and monitor origin) → `cargo test --workspace` PASS.
- [ ] **Step 5: Commit** `feat(i3bar): min_width, align, markup, per-side borders; full click JSON`.

---

### Task 3: Multiple bars in config, reserved space, GET_BAR_CONFIG

**Files:** Modify `crates/core/src/config.rs`, `crates/core/src/arrange.rs`, `crates/core/src/ipc.rs`, and every user of `config.bar` (grep: `crates/i3win/src/app.rs`, `win/bar.rs`)

**Interfaces:**
- Produces:

```rust
// Config: `pub bar: Option<BarConfig>` -> `pub bars: Vec<BarConfig>`
// BarConfig gains:
pub id: String,                 // "bar-N" by file order unless `id NAME`
pub outputs: Vec<String>,       // `output NAME` repeatable; empty = all; values may be "primary" / "nonprimary"
pub mode: BarMode,              // enum BarMode { Dock, Hide, Invisible }   (parsed in Task 5; default Dock)
// arrange.rs
pub fn bar_applies(bar: &BarConfig, output_name: &str, primary: bool) -> bool
pub fn reserved_bar_space(config: &Config, output_name: &str, primary: bool, scale: f32) -> (i32, i32)  // (top, bottom) px: sum of heights of Dock-mode bars that apply
pub fn bar_height_of(bar: &BarConfig, config: &Config, scale: f32) -> i32   // replaces bar_height(config, scale)
// NodeKind::Output gains `primary: bool` (OutputInfo gains `primary: bool`; monitors::enumerate sets it from MONITORINFOF_PRIMARY; testutil::out sets false, first output in Wm::new tests may set true where needed)
// ipc::bar_config_json(wm, payload): "" -> array of all ids; "ID" -> that bar's full i3 shape:
//   {"id","mode","hidden_state","position","status_command","font","workspace_buttons","binding_mode_indicator",
//    "strip_workspace_numbers","strip_workspace_name","separator_symbol","verbose":false,"modifier","outputs","tray_output":null,
//    "colors":{"background","statusline","separator","focused_background","focused_statusline","focused_separator",
//              "focused_workspace_border","focused_workspace_bg","focused_workspace_text", ...active/inactive/urgent/binding_mode...}}
//   unknown id -> {"success":false,"error":"No such bar"}  (fields not yet parsed use their defaults)
```

- Several bars on one output at the same position stack outward from the screen edge in file order (bar-0 closest to the edge). `arrange` insets the workspace area by `reserved_bar_space`. Hide/Invisible bars reserve nothing.

- [ ] **Step 1: Failing tests**

```rust
// config.rs
#[test]
fn multiple_bars_ids_and_outputs() {
    let c = parse_config("bar {\nstatus_command a\n}\nbar {\nid top\nposition top\noutput DISPLAY2\noutput primary\n}\n");
    assert_eq!(c.bars.len(), 2);
    assert_eq!(c.bars[0].id, "bar-0");
    assert_eq!(c.bars[1].id, "top");
    assert_eq!(c.bars[1].outputs, vec!["DISPLAY2", "primary"]);
}
// arrange.rs
#[test]
fn reserved_space_stacks_and_filters() {
    let mut c = parse_config("bar {\n}\nbar {\n}\nbar {\nposition top\noutput B\n}\n");
    c.font = Font { family: "x".into(), size_pt: 9.0 };
    let h = bar_height_of(&c.bars[0], &c, 1.0);
    assert_eq!(reserved_bar_space(&c, "A", false, 1.0), (0, 2 * h));
    assert_eq!(reserved_bar_space(&c, "B", false, 1.0), (h, 2 * h));
    assert!(bar_applies(&parse_config("bar {\noutput primary\n}\n").bars[0], "X", true));
    assert!(!bar_applies(&parse_config("bar {\noutput nonprimary\n}\n").bars[0], "X", true));
}
// ipc.rs
#[test]
fn bar_config_all_ids_and_shape() {
    let wm = Wm::new(parse_config("bar {\n}\nbar {\nid b2\n}\n"), vec![out("O1", 0, 1000)]);
    assert_eq!(bar_config_json(&wm, ""), serde_json::json!(["bar-0", "b2"]));
    let v = bar_config_json(&wm, "b2");
    for k in ["id","mode","hidden_state","position","workspace_buttons","binding_mode_indicator","modifier","colors"] {
        assert!(v.get(k).is_some(), "missing {k}");
    }
    assert_eq!(v["position"], "bottom");
    assert_eq!(bar_config_json(&wm, "nope")["success"], false);
}
```

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS. The platform keeps working with the first bar only until Task 6 (use `config.bars.first()` where `config.bar` was used, plus `reserved_bar_space` in arrange).
- [ ] **Step 5: Commit** `feat(bar): multiple bar blocks, output filter, stacked reserved space, full GET_BAR_CONFIG`.

---

### Task 4: Bar options and the pure bar layout v2

**Files:** Modify `crates/core/src/config.rs` (bar keywords), `crates/i3win/src/bar_layout.rs`

**Interfaces:**
- Consumes: `Block` fields (Task 2), `parse_markup`/`plain_text` (Task 1).
- Produces:

```rust
// BarConfig gains:
pub hidden_state: HiddenState,           // enum HiddenState { Hide, Show }
pub modifier: Option<Modifiers>,         // default Some(Modifiers::WIN); "none" -> None
pub separator_symbol: Option<String>,
pub strip_workspace_numbers: bool, pub strip_workspace_name: bool,
pub binding_mode_indicator: bool,        // default true
pub workspace_min_width: i32,            // px, default 0
pub padding: [i32; 4],                   // top, right, bottom, left px; CSS 1-4 value forms ("2px", "2px 4px", ...), default [0;4]
pub mode_markup: bool,                   // unused here; set by Task 5 for `mode --pango_markup`
pub bindings: Vec<BarBinding>,           // struct BarBinding { pub button: u8, pub release: bool, pub command: String } from `bindsym [--release] buttonN cmd`
// BarColors gains: focused_background, focused_statusline, focused_separator: Option<Color>
// bar_layout.rs
pub struct BarStyle<'a> {
    pub colors: &'a BarColors, pub focused_output: bool, pub show_buttons: bool, pub show_mode: bool,
    pub strip_numbers: bool, pub strip_name: bool, pub workspace_min_width: i32,
    pub separator_symbol: Option<&'a str>, pub padding: [i32; 4],
}
pub fn layout_bar(width: i32, height: i32, scale: f32, buttons: &[WorkspaceButton], mode: Option<&str>,
                  blocks: &[Block], style: &BarStyle, measure: &dyn Fn(&str, bool) -> i32) -> BarLayout
//   measure(text, is_markup) -> px width
// BarItem gains: pub markup: bool, pub borders: [i32; 4] (l,t,r,b), pub text_x: i32 (bar-relative x where text starts; implements align)
// BarLayout.separators becomes Vec<Separator> { x: i32, symbol: Option<String> }
// BarLayout gains: pub background: Color, pub statusline: Color, pub separator_color: Color  (focused_* when focused_output and set)
pub fn display_name(name: &str, num: Option<i32>, strip_numbers: bool, strip_name: bool) -> String
```

- Bar keywords `mode`, `hidden_state`, `modifier`, `separator_symbol`, `strip_workspace_numbers`, `strip_workspace_name`, `binding_mode_indicator`, `workspace_min_width`, `padding`, `tray_output`, `tray_padding` (ignored), `bindsym [--release] buttonN`, colors `focused_background|focused_statusline|focused_separator`. (`BarMode` parsing for `mode` lands here too; Task 7 uses it.)
- `display_name` (i3): strip numbers: `"1: mail"` → `"mail"`, `"3"` stays `"3"` (a name that is only a number is never stripped). Strip name: `"1: mail"` → `"1"`; names without a leading number unchanged.
- Layout rules, right to left for blocks:
  1. Try all blocks with `full_text`. If they don't fit between the left items and the right edge, try all blocks with `short_text` (blocks without short_text keep full_text). If still not, drop blocks from the left (v1) using the short texts.
  2. Block width = max(measured text, min_width) where `MinWidth::Px(n)` = n×scale, `MinWidth::Text(t)` = measure(t). Text placed by `align` inside that width (`text_x`).
  3. Block borders: if `border` set, each side = `border_<side>` or 1 (scaled), else 0; the item rect grows by the borders.
  4. Separators: v1 positions; with `separator_symbol` the gap is `max(separator_block_width, measure(symbol))` and the symbol is drawn centered in the gap.
  5. Workspace buttons: text = `display_name`, width = max(text + 2×pad, workspace_min_width×scale). Buttons and mode indicator respect `show_buttons`/`show_mode`.
  6. `padding` shrinks the content area: items are laid out inside `[left, top, width-right, height-bottom]` (bar height itself is computed in Task 3's `bar_height_of`, which adds top+bottom padding).

- [ ] **Step 1: Failing tests** (in `bar_layout.rs` tests; `measure` = 10 px per char, ignoring markup tags when `is_markup`)

```rust
fn m(t: &str, mk: bool) -> i32 { (if mk { i3win_core::markup::plain_text(t) } else { t.to_string() }).chars().count() as i32 * 10 }
fn blk(full: &str, short: Option<&str>) -> Block { Block { full_text: full.into(), short_text: short.map(Into::into), ..test_block() } }
#[test]
fn short_text_used_before_dropping() {
    let blocks = vec![blk("aaaaaaaaaa", Some("a")), blk("bbbbbbbbbb", Some("b"))];
    let l = layout_bar(150, 20, 1.0, &[], None, &blocks, &style(), &m);
    let texts: Vec<_> = l.items.iter().map(|i| i.text.as_str()).collect();
    assert_eq!(texts, vec!["a", "b"]);
    let l = layout_bar(400, 20, 1.0, &[], None, &blocks, &style(), &m);
    assert_eq!(l.items[0].text, "aaaaaaaaaa");
}
#[test]
fn min_width_and_align() {
    let mut b = blk("ab", None);
    b.min_width = Some(MinWidth::Px(100));
    b.align = Some("right".into());
    let l = layout_bar(400, 20, 1.0, &[], None, &[b], &style(), &m);
    let it = &l.items[0];
    assert_eq!(it.rect.w, 100);
    assert_eq!(it.text_x, it.rect.x + 100 - 20);
    let mut c = blk("ab", None);
    c.min_width = Some(MinWidth::Text("abcdef".into()));
    c.align = Some("center".into());
    let it = &layout_bar(400, 20, 1.0, &[], None, &[c], &style(), &m).items[0];
    assert_eq!((it.rect.w, it.text_x - it.rect.x), (60, 20));
}
#[test]
fn block_border_sides() {
    let mut b = blk("ab", None);
    b.border = Some("#ff0000".into());
    b.border_left = Some(0);
    b.border_bottom = Some(3);
    let it = &layout_bar(400, 20, 1.0, &[], None, &[b], &style(), &m).items[0];
    assert_eq!(it.borders, [0, 1, 1, 3]);
}
#[test]
fn workspace_names_stripped_and_min_width() {
    assert_eq!(display_name("1: mail", Some(1), true, false), "mail");
    assert_eq!(display_name("3", Some(3), true, false), "3");
    assert_eq!(display_name("1: mail", Some(1), false, true), "1");
    let mut st = style();
    st.workspace_min_width = 80;
    let buttons = vec![WorkspaceButton { name: "1".into(), focused: true, visible: true, urgent: false }];
    let l = layout_bar(400, 20, 1.0, &buttons, None, &[], &st, &m);
    assert_eq!(l.items[0].rect.w, 80);
}
#[test]
fn separator_symbol_and_focused_colors() {
    let mut st = style();
    st.separator_symbol = Some("|");
    let l = layout_bar(400, 20, 1.0, &[], None, &[blk("a", None), blk("b", None)], &st, &m);
    assert_eq!(l.separators[0].symbol.as_deref(), Some("|"));
}
#[test]
fn bar_options_parse() {
    let c = parse_config("bar {\nmode hide\nhidden_state show\nmodifier Mod1\nseparator_symbol \" | \"\nstrip_workspace_numbers yes\nbinding_mode_indicator no\nworkspace_min_width 40\npadding 2px 4px\ntray_output primary\nbindsym button4 workspace prev\nbindsym --release button3 nop\ncolors {\nfocused_background #111111\n}\n}\n");
    let b = &c.bars[0];
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    assert_eq!(b.mode, BarMode::Hide);
    assert_eq!(b.hidden_state, HiddenState::Show);
    assert_eq!(b.modifier, Some(Modifiers::ALT));
    assert_eq!(b.separator_symbol.as_deref(), Some(" | "));
    assert!(b.strip_workspace_numbers && !b.binding_mode_indicator);
    assert_eq!(b.workspace_min_width, 40);
    assert_eq!(b.padding, [2, 4, 2, 4]);
    assert_eq!(b.bindings.len(), 2);
    assert!(b.bindings[1].release);
    assert_eq!(b.colors.focused_background, Some(Color(0x11, 0x11, 0x11)));
}
```

(`style()` and `test_block()` are small test helpers you write: default `BarStyle` with `BarColors::default()`, all flags as i3 defaults; `Block` with v1 defaults.)

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS (update `win/bar.rs` call sites to the new `layout_bar` signature, drawing still plain until Task 5).
- [ ] **Step 5: Commit** `feat(bar): i3 bar options and layout (short_text, min_width, align, borders, symbol, strip)`.

---

### Task 5: GDI markup rendering for bar blocks and titles

**Files:** Modify `crates/i3win/src/win/gdi.rs`, `crates/i3win/src/win/bar.rs`, `crates/i3win/src/win/deco.rs`, `crates/core/src/title.rs`, `crates/core/src/arrange.rs`, `crates/core/src/config.rs`

**Interfaces:**
- Consumes: `parse_markup`, `Run`, `Style` (Task 1); `BarItem.markup/text_x/borders` (Task 4).
- Produces:

```rust
// gdi.rs
pub fn measure_runs(hdc: HDC, runs: &[Run], base: &Font, scale: f32) -> i32
pub fn draw_runs(hdc: HDC, x: i32, r: Rect, runs: &[Run], default_fg: Color, base: &Font, scale: f32)  // vertically centered in r, clipped to r
// title.rs: format_title escapes the substituted %title/%class/%instance/%machine values with escape_markup when `format` is Some (the format text itself is markup); returns (String, bool /*is_markup*/)
pub fn format_title(...same params...) -> (String, bool)
// arrange.rs Deco::Frame and Tab gain: pub markup: bool
// config.rs: `mode [--pango_markup] "name" {` sets BarConfig-independent `Config.mode_markup: HashSet<String>`; the bar draws that mode name as markup
```

- Each run: font = base family (or `style.family`, through `resolve_font_family`; `mono` → Consolas), size = `size_pt` or base size × `scale`, weight = `weight` or 700 if bold, italic/underline/strike flags; fonts come from `gdi::font`'s cache (extend its key with weight/italic/underline/strike). `bg` fills the run's text extent first. `sup`/`sub` are drawn smaller (scale 0.75) on the same baseline (no rise; acceptable).
- Bar: blocks with `markup == "pango"` are drawn with `draw_runs`; others with v1 `draw_text`. Block borders drawn per side. Separator symbol drawn with `statusline`/`separator` color centered at `Separator.x`. Mode indicator uses markup if `config.mode_markup` contains it.
- Deco: titles with `markup == true` drawn with `draw_runs`; tab bars likewise.

- [ ] **Step 1: Failing tests** (core; GDI drawing is checked by the manual checklist)

```rust
// title.rs
#[test]
fn title_format_escapes_values_and_flags_markup() {
    let (t, mk) = format_title(Some("<b>%title</b>"), "a<b>&", "c", "i", "PC", &[], true);
    assert_eq!(t, "<b>a&lt;b&gt;&amp;</b>");
    assert!(mk);
    let (t, mk) = format_title(None, "a<b>", "c", "i", "PC", &[], true);
    assert_eq!(t, "a<b>");
    assert!(!mk);
}
// config.rs
#[test]
fn mode_pango_markup_flag() {
    let c = parse_config("mode --pango_markup \"<b>resize</b>\" {\nbindsym Escape mode default\n}\n");
    assert!(c.mode_markup.contains("<b>resize</b>"));
    assert!(c.bindings.contains_key("<b>resize</b>"));
}
```

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS; `cargo build --workspace`.
- [ ] **Step 5: Commit** `feat(gdi): markup text runs in bar blocks, titles and mode indicator`.

---

### Task 6: Platform: one window set and one status process per bar

**Files:** Modify `crates/i3win/src/app.rs`, `crates/i3win/src/win/bar.rs`, `crates/i3win/src/mainloop.rs`, `crates/i3win/src/win/monitors.rs`

**Interfaces:**
- Consumes: `Config.bars`, `bar_applies`, `bar_height_of`, `reserved_bar_space` (Task 3), `layout_bar`/`BarStyle` (Task 4).
- Produces:

```rust
// app.rs
struct BarStatus { process: Option<StatusProcess>, blocks: Vec<Block> }
statuses: HashMap<String /*bar id*/, BarStatus>   // replaces `status` and `blocks`
// MainMsg::StatusUpdate(String /*bar id*/, u64, Vec<Block>), MainMsg::StatusExited(String, u64)
// StatusProcess::spawn(command: &str, bar_id: String) -> io::Result<StatusProcess>  (posts messages tagged with bar_id)
// win/bar.rs
pub fn bar_rect_stacked(o: Rect, position: BarPosition, offset: i32, h: i32) -> Rect   // offset = heights of earlier bars at the same edge
impl Bars { pub fn update(&mut self, wm: &Wm, arr: &Arrangement, blocks: &HashMap<String, Vec<Block>>) }  // windows keyed by (bar id, output name)
pub fn workspace_at(&self, x: i32, y: i32) -> Option<String>    // screen point -> workspace name under a visible bar's workspace button (Plan 8 drop target)
```

- Restart all status processes on reload (v1 `restart_status` per bar). A click on a bar window posts `StatusClick { bar: <that bar's id>, click }` and only that bar's process receives it. Bars whose `outputs` don't match a monitor get no windows; `refresh_outputs` re-evaluates.

- [ ] **Step 1: Failing tests** (pure helpers)

```rust
#[test]
fn bar_rect_stacking() {
    let o = Rect::new(0, 0, 1000, 500);
    assert_eq!(bar_rect_stacked(o, BarPosition::Bottom, 0, 20), Rect::new(0, 480, 1000, 20));
    assert_eq!(bar_rect_stacked(o, BarPosition::Bottom, 20, 20), Rect::new(0, 460, 1000, 20));
    assert_eq!(bar_rect_stacked(o, BarPosition::Top, 20, 20), Rect::new(0, 20, 1000, 20));
}
#[test]
fn bar_outputs_filter() {
    let c = parse_config("bar {\noutput NOPE\n}\n");
    assert!(!bar_applies(&c.bars[0], "DISPLAY1", true));
}
#[test]
fn click_routes_to_owning_bar() {
    // route_click is a pure fn in app.rs: picks the status entry by bar id
    let mut ids = HashMap::new();
    ids.insert("bar-0".to_string(), 0u8);
    ids.insert("bar-1".to_string(), 1u8);
    assert_eq!(route_click(&ids, "bar-1"), Some(&1));
    assert_eq!(route_click(&ids, "bar-9"), None);
}
```

(`pub(crate) fn route_click<'a, T>(statuses: &'a HashMap<String, T>, bar: &str) -> Option<&'a T>`.)

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS; `cargo build --workspace`.
- [ ] **Step 5: Commit** `feat(bar): multiple bars on the platform side, per-bar status processes`.

---

### Task 7: Hide mode, `bar` commands, barconfig_update

**Files:** Modify `crates/core/src/command.rs`, `crates/core/src/wm.rs`, `crates/core/src/ipc.rs`, `crates/core/src/arrange.rs`, `crates/i3win/src/win/keyboard.rs`, `crates/i3win/src/mainloop.rs`, `crates/i3win/src/app.rs`, `crates/i3win/src/win/bar.rs`

**Interfaces:**
- Produces:

```rust
// command.rs
Command::BarMode { mode: BarModeArg, id: Option<String> }          // "bar mode dock|hide|invisible|toggle [ID]"
Command::BarHiddenState { state: HiddenArg, id: Option<String> }   // "bar hidden_state hide|show|toggle [ID]"
// wm.rs: runtime state, initialised from config on start/reload
pub bar_state: HashMap<String, (BarMode, HiddenState)>
pub fn bar_state_of(&self, id: &str) -> (BarMode, HiddenState)
IpcEvent::BarconfigUpdate { id: String }   // event_json -> EV_BARCONFIG_UPDATE with the bar's full GET_BAR_CONFIG object (with runtime mode/hidden_state)
// keyboard.rs
pub fn set_modifier_watch(mods: Option<Modifiers>)   // Some(m): hook posts WM_APP_MODS when "all of m held" flips; None: no posts at all
// mainloop: WM_APP_MODS -> App::on_modifier_state(held: bool)
```

- `toggle` for mode: Dock ↔ Hide (i3). `toggle` for hidden_state: Hide ↔ Show. No ID = all bars. Unknown ID = error `"No such bar"`.
- Visibility of a bar in Hide mode: shown iff hidden_state Show, or modifier held, or any workspace on its output is urgent. Invisible: never shown. Hide/Invisible reserve no space (`reserved_bar_space` uses `bar_state` mode: compute it from `Wm` — change `reserved_bar_space` to take a `&dyn Fn(&str) -> BarMode`, and `arrange` passes `wm.bar_state_of`). When showing a hide-mode bar, raise it topmost; when hiding, `SW_HIDE`. Modifier changes only redraw bars (no flush).
- `set_modifier_watch` is set on every flush to `Some(m)` where m is the modifier of any bar in Hide mode (if several differ, use the first), else `None`.

- [ ] **Step 1: Failing tests**

```rust
// wm / command
#[test]
fn bar_commands_change_state_and_emit() {
    let mut wm = Wm::new(parse_config("bar {\n}\nbar {\nid b\nmode hide\n}\n"), vec![out("O1", 0, 1000)]);
    wm.take_effects();
    assert!(wm.run_command("bar mode toggle bar-0")[0].success);
    assert_eq!(wm.bar_state_of("bar-0").0, BarMode::Hide);
    assert!(wm.run_command("bar hidden_state show")[0].success);
    assert_eq!(wm.bar_state_of("b").1, HiddenState::Show);
    assert!(!wm.run_command("bar mode dock nope")[0].success);
    let n = wm.take_effects().iter().filter(|e| matches!(e, Effect::Ipc(IpcEvent::BarconfigUpdate { .. }))).count();
    assert_eq!(n, 3); // bar-0 once, then both bars for the id-less hidden_state
}
#[test]
fn hide_mode_bar_reserves_no_space() {
    let mut cfg = parse_config("bar {\nmode hide\n}\ndefault_border none\n");
    cfg.font = Font { family: "x".into(), size_pt: 9.0 };
    let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
    wm.manage(win(1));
    assert_eq!(wm.arrange().windows[0].rect.h, 500);
    wm.run_command("bar mode dock");
    assert!(wm.arrange().windows[0].rect.h < 500);
}
// keyboard.rs
#[test]
fn modifier_watch_disabled_without_hide_bars() {
    let mut k = KeyMatcher::new();
    k.set_watch(None);
    assert_eq!(k.watch_flip(0x5B, true), None);
    k.set_watch(Some(Modifiers::WIN));
    assert_eq!(k.watch_flip(0x5B, true), Some(true));
    assert_eq!(k.watch_flip(0x5B, true), None);  // auto-repeat: no flip
    assert_eq!(k.watch_flip(0x5B, false), Some(false));
}
```

(`KeyMatcher::set_watch(Option<Modifiers>)` and `KeyMatcher::watch_flip(vk, down) -> Option<bool>` are the pure parts behind `set_modifier_watch`; the hook posts `WM_APP_MODS` only when `watch_flip` returns Some.)

- [ ] **Step 2–4:** FAIL → implement → `cargo test --workspace` PASS.
- [ ] **Step 5: Commit** `feat(bar): hide mode, bar commands, barconfig_update`.

---

### Task 8: Bar button bindings, live checks, docs

**Files:** Modify `crates/i3win/src/win/bar.rs`, `crates/i3win/src/mainloop.rs`, `scripts/live-checks.ps1`, `docs/manual-checks.md`

**Interfaces:**
- Consumes: `BarConfig.bindings` (Task 4).
- Produces: `pub fn bar_binding_for(bindings: &[BarBinding], button: u8, release: bool) -> Option<&str>` (pure, in `bar_layout.rs`).
- Mouse buttons on the bar map to i3's numbers: left 1, middle 2, right 3, wheel up 4, wheel down 5, wheel left 6, wheel right 7 (`WM_MOUSEHWHEEL`). On press (and on release for `--release` entries) a bound button runs its command via `MainMsg::RunCommand`, and the default action (workspace click / v1 wheel prev-next / block click) does not run. Unbound buttons keep the defaults.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn bar_binding_lookup() {
    let b = vec![BarBinding { button: 4, release: false, command: "workspace prev".into() },
                 BarBinding { button: 3, release: true, command: "nop".into() }];
    assert_eq!(bar_binding_for(&b, 4, false), Some("workspace prev"));
    assert_eq!(bar_binding_for(&b, 3, false), None);
    assert_eq!(bar_binding_for(&b, 3, true), Some("nop"));
}
```

- [ ] **Step 2–4:** FAIL → implement → PASS.
- [ ] **Step 5: Live checks & docs.** Append to `scripts/live-checks.ps1`: the bar window (class `i3win_bar`) of the default config sits at the bottom of the work area; `i3win-msg -t get_bar_config` returns `["bar-0"]`; `i3win-msg bar mode hide` → the bar window becomes invisible and the Notepads' height grows; `bar mode dock` restores. Add "Plan 6" items to `docs/manual-checks.md`: a status script emitting `markup: "pango"` with `<b>`/colored spans renders bold/colored; `short_text` kicks in on a narrow monitor; `min_width` + `align`; `separator_symbol " | "`; `strip_workspace_numbers yes` with `workspace "1: mail"`; two `bar {}` blocks stack; `mode hide` shows the bar only while Win is held; `bindsym button3 exec notepad` in the bar opens Notepad on right click; a `title_format "<b>%title</b>"` renders bold titles.
- [ ] **Step 6: Commit** `feat(bar): button bindings; live checks and manual checks`.
