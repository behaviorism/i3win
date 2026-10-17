//! Pure bar layout: positions workspace buttons, the mode indicator and status
//! blocks, with text measurement injected so it is testable without GDI.
use i3win_core::bar_model::WorkspaceButton;
use i3win_core::config::{parse_color, BarColorTriple, BarColors, Color};
use i3win_core::geom::Rect;
use i3win_core::i3bar::{Block, MinWidth};

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Workspace(String),
    Mode,
    Block(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarItem {
    pub kind: ItemKind,
    pub rect: Rect,
    pub text: String,
    pub fg: Color,
    pub bg: Option<Color>,
    pub border: Option<Color>,
    /// Whether `text` is pango markup.
    pub markup: bool,
    /// Border widths in px: left, top, right, bottom.
    pub borders: [i32; 4],
    /// Bar-relative x where the text starts (implements `align`).
    pub text_x: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Separator {
    /// Bar-relative x of the gap center.
    pub x: i32,
    pub symbol: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarLayout {
    pub items: Vec<BarItem>,
    pub separators: Vec<Separator>,
    pub background: Color,
    pub statusline: Color,
    pub separator_color: Color,
    /// The padded content area (bar-relative) that items and separators live in.
    pub content: Rect,
}

pub struct BarStyle<'a> {
    pub colors: &'a BarColors,
    pub focused_output: bool,
    pub show_buttons: bool,
    pub show_mode: bool,
    pub strip_numbers: bool,
    pub strip_name: bool,
    pub workspace_min_width: i32,
    pub separator_symbol: Option<&'a str>,
    /// Whether the mode indicator text is markup (`mode --pango_markup`).
    pub mode_markup: bool,
    /// top, right, bottom, left in px.
    pub padding: [i32; 4],
}

/// The workspace number a name starts with, if any.
pub fn leading_number(name: &str) -> Option<i32> {
    let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// i3's workspace button text. A name that is only a number is never stripped.
pub fn display_name(name: &str, num: Option<i32>, strip_numbers: bool, strip_name: bool) -> String {
    let Some(n) = num else {
        return name.to_string();
    };
    if strip_name {
        return n.to_string();
    }
    if strip_numbers {
        let rest = name.trim_start_matches(|c: char| c.is_ascii_digit());
        let rest = rest.trim_start_matches(':').trim_start();
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    name.to_string()
}

fn triple_item(
    kind: ItemKind,
    rect: Rect,
    text: &str,
    t: &BarColorTriple,
    text_x: i32,
    markup: bool,
) -> BarItem {
    BarItem {
        kind,
        rect,
        text: text.to_string(),
        fg: t.text,
        bg: Some(t.background),
        border: Some(t.border),
        markup,
        borders: [1, 1, 1, 1],
        text_x,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn layout_bar(
    width: i32,
    height: i32,
    scale: f32,
    buttons: &[WorkspaceButton],
    mode: Option<&str>,
    blocks: &[Block],
    style: &BarStyle,
    measure: &dyn Fn(&str, bool) -> i32,
) -> BarLayout {
    let s = |v: i32| (v as f32 * scale).round() as i32;
    let colors = style.colors;
    let focused = style.focused_output;
    let background = colors
        .focused_background
        .filter(|_| focused)
        .unwrap_or(colors.background);
    let statusline = colors
        .focused_statusline
        .filter(|_| focused)
        .unwrap_or(colors.statusline);
    let separator_color = colors
        .focused_separator
        .filter(|_| focused)
        .unwrap_or(colors.separator);

    let [pt, pr, pb, pl] = style.padding.map(|p| s(p).max(0));
    let top = pt;
    let item_h = height.saturating_sub(pt).saturating_sub(pb);
    let content_right = width.saturating_sub(pr);

    let pad = s(5).max(0);
    let gap1 = s(1).max(0);
    let mut items = Vec::new();
    let mut x = pl;

    let mut left =
        |kind: ItemKind, text: &str, markup: bool, min_w: i32, t: &BarColorTriple, x: &mut i32| {
            let tw = measure(text, markup).max(0);
            let w = tw.saturating_add(pad.saturating_mul(2)).max(min_w);
            let text_x = x.saturating_add((w - tw) / 2);
            items.push(triple_item(
                kind,
                Rect::new(*x, top, w, item_h),
                text,
                t,
                text_x,
                markup,
            ));
            *x = x.saturating_add(w).saturating_add(gap1);
        };
    if style.show_buttons {
        let min_w = s(style.workspace_min_width).max(0);
        for b in buttons {
            let t = if b.urgent {
                &colors.urgent_workspace
            } else if b.focused {
                &colors.focused_workspace
            } else if b.visible {
                &colors.active_workspace
            } else {
                &colors.inactive_workspace
            };
            let text = display_name(
                &b.name,
                leading_number(&b.name),
                style.strip_numbers,
                style.strip_name,
            );
            left(
                ItemKind::Workspace(b.name.clone()),
                &text,
                false,
                min_w,
                t,
                &mut x,
            );
        }
    }
    if let Some(m) = mode.filter(|m| style.show_mode && *m != "default") {
        left(
            ItemKind::Mode,
            m,
            style.mode_markup,
            0,
            &colors.binding_mode,
            &mut x,
        );
    }
    let left_end = x;

    // Places blocks right to left; reports whether any block did not fit.
    let place = |use_short: bool| -> (Vec<BarItem>, Vec<Separator>, bool) {
        let mut placed: Vec<BarItem> = Vec::new();
        let mut separators = Vec::new();
        let mut prev_x: Option<i32> = None;
        let mut dropped = false;
        for (i, block) in blocks.iter().enumerate().rev() {
            let text = if use_short {
                block.short_text.as_deref().unwrap_or(&block.full_text)
            } else {
                &block.full_text
            };
            let markup = block.markup.as_deref() == Some("pango");
            let cw = measure(text, markup).max(0);
            let min_w = match &block.min_width {
                Some(MinWidth::Px(n)) => s(*n),
                Some(MinWidth::Text(t)) => measure(t, markup),
                None => 0,
            };
            let w = cw.max(min_w);
            let has_border = block.border.is_some() || block.urgent;
            let side = |v: Option<i32>| {
                if has_border {
                    s(v.unwrap_or(1)).max(0)
                } else {
                    0
                }
            };
            let borders = [
                side(block.border_left),
                side(block.border_top),
                side(block.border_right),
                side(block.border_bottom),
            ];
            let total = w.saturating_add(borders[0]).saturating_add(borders[2]);
            let mut gap = s(block.separator_block_width.unwrap_or(9)).max(0);
            let symbol = style.separator_symbol.filter(|_| block.separator);
            if let Some(sym) = symbol {
                gap = gap.max(measure(sym, false));
            }
            let right = match prev_x {
                None => content_right.saturating_sub(s(4)),
                Some(px) => px.saturating_sub(gap),
            };
            if right.saturating_sub(total) < left_end {
                dropped = true;
                break;
            }
            let rect = Rect::new(right - total, top, total, item_h);
            let base = rect.x + borders[0];
            let text_x = match block.align.as_deref() {
                Some("center") => base + (w - cw) / 2,
                Some("right") => base + (w - cw),
                _ => base,
            };
            let mut item = if block.urgent {
                triple_item(
                    ItemKind::Block(i),
                    rect,
                    text,
                    &colors.urgent_workspace,
                    text_x,
                    markup,
                )
            } else {
                BarItem {
                    kind: ItemKind::Block(i),
                    rect,
                    text: text.to_string(),
                    fg: block
                        .color
                        .as_deref()
                        .and_then(parse_color)
                        .unwrap_or(statusline),
                    bg: block.background.as_deref().and_then(parse_color),
                    border: block.border.as_deref().and_then(parse_color),
                    markup,
                    borders,
                    text_x,
                }
            };
            item.borders = borders;
            item.markup = markup;
            if let Some(px) = prev_x {
                if block.separator {
                    separators.push(Separator {
                        x: px.saturating_sub(gap.saturating_add(1) / 2),
                        symbol: symbol.map(str::to_string),
                    });
                }
            }
            prev_x = Some(rect.x);
            placed.push(item);
        }
        placed.reverse();
        separators.reverse();
        (placed, separators, dropped)
    };
    let (mut placed, mut separators, dropped) = place(false);
    if dropped {
        (placed, separators, _) = place(true);
    }
    items.extend(placed);
    BarLayout {
        items,
        separators,
        background,
        statusline,
        separator_color,
        content: Rect::new(
            pl,
            top,
            content_right.saturating_sub(pl).max(0),
            item_h.max(0),
        ),
    }
}

pub fn click_command(name: &str) -> String {
    let mut out = String::from("workspace \"");
    for c in name.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

pub fn hit_test(layout: &BarLayout, x: i32, y: i32) -> Option<&BarItem> {
    layout.items.iter().find(|i| {
        let r = &i.rect;
        x >= r.x && x < r.right() && y >= r.y && y < r.y.saturating_add(r.h)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use i3win_core::config::BarConfig;
    use std::sync::OnceLock;

    /// 7 px per char (v1 tests).
    fn m7(s: &str, _mk: bool) -> i32 {
        7 * s.chars().count() as i32
    }
    /// 10 px per char, ignoring markup tags when `mk`.
    fn m(t: &str, mk: bool) -> i32 {
        (if mk {
            i3win_core::markup::plain_text(t)
        } else {
            t.to_string()
        })
        .chars()
        .count() as i32
            * 10
    }
    fn colors() -> &'static BarColors {
        static C: OnceLock<BarColors> = OnceLock::new();
        C.get_or_init(|| BarConfig::default().colors)
    }
    fn style() -> BarStyle<'static> {
        BarStyle {
            colors: colors(),
            focused_output: false,
            show_buttons: true,
            show_mode: true,
            strip_numbers: false,
            strip_name: false,
            workspace_min_width: 0,
            separator_symbol: None,
            mode_markup: false,
            padding: [0; 4],
        }
    }
    fn btn(n: &str, f: bool, v: bool) -> WorkspaceButton {
        WorkspaceButton {
            name: n.into(),
            focused: f,
            visible: v,
            urgent: false,
        }
    }
    fn test_block() -> Block {
        Block {
            separator: true,
            ..Default::default()
        }
    }
    fn blk(full: &str, short: Option<&str>) -> Block {
        Block {
            full_text: full.into(),
            short_text: short.map(Into::into),
            ..test_block()
        }
    }

    #[test]
    fn buttons_left() {
        let c = colors();
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[btn("1", false, false), btn("2", true, true)],
            None,
            &[],
            &style(),
            &m7,
        );
        assert_eq!(l.items[0].rect, Rect::new(0, 0, 17, 22));
        assert_eq!(l.items[1].rect, Rect::new(18, 0, 17, 22));
        assert_eq!(l.items[1].bg, Some(c.focused_workspace.background));
        assert_eq!(l.items[0].bg, Some(c.inactive_workspace.background));
    }
    #[test]
    fn mode_markup_is_measured_and_flagged() {
        let st = BarStyle {
            mode_markup: true,
            ..style()
        };
        let l = layout_bar(1000, 22, 1.0, &[], Some("<b>ab</b>"), &[], &st, &m);
        assert!(l.items[0].markup);
        // 2 visible chars x 10 px + 2 x 5 px padding.
        assert_eq!(l.items[0].rect.w, 30);
    }
    #[test]
    fn mode_indicator_after_buttons() {
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[btn("1", true, true)],
            Some("resize"),
            &[],
            &style(),
            &m7,
        );
        assert_eq!(l.items[1].kind, ItemKind::Mode);
        assert_eq!(l.items[1].rect, Rect::new(18, 0, 52, 22));
        assert!(
            layout_bar(1000, 22, 1.0, &[], Some("default"), &[], &style(), &m7)
                .items
                .is_empty()
        );
        let mut st = style();
        st.show_mode = false;
        assert!(
            layout_bar(1000, 22, 1.0, &[], Some("resize"), &[], &st, &m7)
                .items
                .is_empty()
        );
    }
    #[test]
    fn blocks_right_aligned_with_separators() {
        let c = colors();
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[],
            None,
            &[blk("ab", None), blk("cde", None)],
            &style(),
            &m7,
        );
        assert_eq!(l.items[1].rect, Rect::new(975, 0, 21, 22));
        assert_eq!(l.items[0].rect, Rect::new(952, 0, 14, 22));
        assert_eq!(l.separators.len(), 1);
        assert_eq!(l.separators[0].x, 970);
        assert_eq!(l.items[1].fg, c.statusline);
    }
    #[test]
    fn status_never_overlaps_buttons() {
        let l = layout_bar(
            100,
            22,
            1.0,
            &[btn("1", true, true)],
            None,
            &[blk("aaaaaaaaaa", None), blk("bbbbbb", None)],
            &style(),
            &m7,
        );
        assert_eq!(l.items.len(), 2);
        assert!(l.items.iter().all(|i| i.kind != ItemKind::Block(0)));
    }
    #[test]
    fn hit_testing() {
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[btn("1", true, true)],
            None,
            &[],
            &style(),
            &m7,
        );
        assert_eq!(
            hit_test(&l, 5, 5).unwrap().kind,
            ItemKind::Workspace("1".into())
        );
        assert!(hit_test(&l, 500, 5).is_none());
    }
    #[test]
    fn click_command_quotes_name() {
        assert_eq!(click_command(r#"my "ws""#), r#"workspace "my \"ws\"""#);
    }
    #[test]
    fn click_command_round_trips_through_the_parser() {
        use i3win_core::command::{parse_commands, Command, WorkspaceTarget};
        for name in [r#"my "ws""#, r"a\b", r"x\", "plain"] {
            let groups = parse_commands(&click_command(name)).unwrap();
            assert_eq!(
                groups[0].commands,
                vec![Command::Workspace {
                    target: WorkspaceTarget::Name(name.into()),
                    no_auto_back_and_forth: false
                }]
            );
        }
    }
    #[test]
    fn odd_inputs_do_not_panic() {
        let mut b = blk("x", None);
        b.separator_block_width = Some(i32::MAX);
        let many: Vec<Block> = (0..10_000).map(|_| b.clone()).collect();
        let mut st = style();
        st.padding = [i32::MAX, i32::MAX, i32::MIN, 5];
        st.separator_symbol = Some("|");
        for w in [0, -5, i32::MIN, i32::MAX] {
            for stl in [&style(), &st] {
                let _ = layout_bar(
                    w,
                    22,
                    1.0,
                    &[btn("1", true, true)],
                    Some("r"),
                    &many,
                    stl,
                    &m7,
                );
                let _ = layout_bar(w, 22, 1e9, &[btn("1", true, true)], None, &many, stl, &m7);
            }
        }
        let _ = layout_bar(100, 22, 1.0, &[], None, &many, &style(), &|_, _| i32::MAX);
    }

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
        assert_eq!(it.rect.w, 20 + 1);
    }
    #[test]
    fn workspace_names_stripped_and_min_width() {
        assert_eq!(display_name("1: mail", Some(1), true, false), "mail");
        assert_eq!(display_name("3", Some(3), true, false), "3");
        assert_eq!(display_name("1: mail", Some(1), false, true), "1");
        assert_eq!(display_name("web", None, true, true), "web");
        let mut st = style();
        st.workspace_min_width = 80;
        let buttons = vec![btn("1", true, true)];
        let l = layout_bar(400, 20, 1.0, &buttons, None, &[], &st, &m);
        assert_eq!(l.items[0].rect.w, 80);
        st.workspace_min_width = 0;
        st.strip_numbers = true;
        let buttons = vec![btn("1: mail", true, true)];
        let l = layout_bar(400, 20, 1.0, &buttons, None, &[], &st, &m);
        assert_eq!(l.items[0].text, "mail");
        assert_eq!(l.items[0].rect.w, 40 + 10);
    }
    #[test]
    fn separator_symbol_and_focused_colors() {
        let mut st = style();
        st.separator_symbol = Some("|");
        let l = layout_bar(
            400,
            20,
            1.0,
            &[],
            None,
            &[blk("a", None), blk("b", None)],
            &st,
            &m,
        );
        assert_eq!(l.separators[0].symbol.as_deref(), Some("|"));

        let colors = BarColors {
            focused_background: Some(Color(1, 2, 3)),
            ..BarColors::default()
        };
        let mut st = style();
        st.colors = &colors;
        assert_eq!(
            layout_bar(400, 20, 1.0, &[], None, &[], &st, &m).background,
            colors.background
        );
        st.focused_output = true;
        let l = layout_bar(400, 20, 1.0, &[], None, &[], &st, &m);
        assert_eq!(l.background, Color(1, 2, 3));
        assert_eq!(l.statusline, colors.statusline);
    }
    #[test]
    fn symbol_widens_gap_and_padding_shrinks_area() {
        let mut st = style();
        st.separator_symbol = Some("<-->");
        let blocks = [blk("a", None), blk("b", None)];
        let l = layout_bar(400, 20, 1.0, &[], None, &blocks, &st, &m);
        // gap = max(9, 40)
        assert_eq!(l.items[1].rect.x - l.items[0].rect.right(), 40);
        let mut st = style();
        st.padding = [2, 4, 3, 6];
        let l = layout_bar(
            400,
            20,
            1.0,
            &[btn("1", true, true)],
            None,
            &blocks,
            &st,
            &m,
        );
        assert_eq!(l.items[0].rect.x, 6);
        assert_eq!((l.items[0].rect.y, l.items[0].rect.h), (2, 15));
        assert_eq!(l.items[2].rect.right(), 400 - 4 - 4);
    }
}
