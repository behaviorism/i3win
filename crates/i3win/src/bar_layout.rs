//! Pure bar layout: positions workspace buttons, the mode indicator and status
//! blocks, with text measurement injected so it is testable without GDI.
use i3win_core::bar_model::WorkspaceButton;
use i3win_core::config::{parse_color, BarColorTriple, BarColors, Color};
use i3win_core::geom::Rect;
use i3win_core::i3bar::Block;

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
}

#[derive(Clone, Debug, PartialEq)]
pub struct BarLayout {
    pub items: Vec<BarItem>,
    /// Separator line x positions (bar-relative).
    pub separators: Vec<i32>,
}

fn triple_item(kind: ItemKind, rect: Rect, text: &str, t: &BarColorTriple) -> BarItem {
    BarItem {
        kind,
        rect,
        text: text.to_string(),
        fg: t.text,
        bg: Some(t.background),
        border: Some(t.border),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn layout_bar(
    width: i32,
    height: i32,
    scale: f32,
    buttons: &[WorkspaceButton],
    show_buttons: bool,
    mode: Option<&str>,
    blocks: &[Block],
    colors: &BarColors,
    measure: &dyn Fn(&str) -> i32,
) -> BarLayout {
    let s = |v: i32| (v as f32 * scale).round() as i32;
    let pad = s(5).max(0);
    let gap1 = s(1).max(0);
    let mut items = Vec::new();
    let mut x = 0i32;

    let mut left = |kind: ItemKind, text: &str, t: &BarColorTriple, x: &mut i32| {
        let w = measure(text).max(0).saturating_add(pad.saturating_mul(2));
        items.push(triple_item(kind, Rect::new(*x, 0, w, height), text, t));
        *x = x.saturating_add(w).saturating_add(gap1);
    };
    if show_buttons {
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
            left(ItemKind::Workspace(b.name.clone()), &b.name, t, &mut x);
        }
    }
    if let Some(m) = mode.filter(|m| *m != "default") {
        left(ItemKind::Mode, m, &colors.binding_mode, &mut x);
    }
    let left_end = x;

    let mut placed: Vec<BarItem> = Vec::new();
    let mut separators = Vec::new();
    let mut prev_x: Option<i32> = None;
    for (i, block) in blocks.iter().enumerate().rev() {
        let gap = s(block.separator_block_width.unwrap_or(9)).max(0);
        let right = match prev_x {
            None => width.saturating_sub(s(4)),
            Some(px) => px.saturating_sub(gap),
        };
        let w = measure(&block.full_text).max(0);
        if right.saturating_sub(w) < left_end {
            break;
        }
        let rect = Rect::new(right - w, 0, w, height);
        let item = if block.urgent {
            let mut it = triple_item(
                ItemKind::Block(i),
                rect,
                &block.full_text,
                &colors.urgent_workspace,
            );
            it.border = Some(colors.urgent_workspace.border);
            it
        } else {
            BarItem {
                kind: ItemKind::Block(i),
                rect,
                text: block.full_text.clone(),
                fg: block
                    .color
                    .as_deref()
                    .and_then(parse_color)
                    .unwrap_or(colors.statusline),
                bg: block.background.as_deref().and_then(parse_color),
                border: block.border.as_deref().and_then(parse_color),
            }
        };
        if let Some(px) = prev_x {
            if block.separator {
                separators.push(px.saturating_sub(gap.saturating_add(1) / 2));
            }
        }
        prev_x = Some(rect.x);
        placed.push(item);
    }
    placed.reverse();
    separators.reverse();
    items.extend(placed);
    BarLayout { items, separators }
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

    fn m(s: &str) -> i32 {
        7 * s.chars().count() as i32
    }
    fn btn(n: &str, f: bool, v: bool) -> WorkspaceButton {
        WorkspaceButton {
            name: n.into(),
            focused: f,
            visible: v,
            urgent: false,
        }
    }
    fn blk(t: &str) -> Block {
        Block {
            full_text: t.into(),
            separator: true,
            ..Default::default()
        }
    }
    #[test]
    fn buttons_left() {
        let c = BarConfig::default().colors;
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[btn("1", false, false), btn("2", true, true)],
            true,
            None,
            &[],
            &c,
            &m,
        );
        assert_eq!(l.items[0].rect, Rect::new(0, 0, 17, 22));
        assert_eq!(l.items[1].rect, Rect::new(18, 0, 17, 22));
        assert_eq!(l.items[1].bg, Some(c.focused_workspace.background));
        assert_eq!(l.items[0].bg, Some(c.inactive_workspace.background));
    }
    #[test]
    fn mode_indicator_after_buttons() {
        let c = BarConfig::default().colors;
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[btn("1", true, true)],
            true,
            Some("resize"),
            &[],
            &c,
            &m,
        );
        assert_eq!(l.items[1].kind, ItemKind::Mode);
        assert_eq!(l.items[1].rect, Rect::new(18, 0, 52, 22));
        assert!(
            layout_bar(1000, 22, 1.0, &[], true, Some("default"), &[], &c, &m)
                .items
                .is_empty()
        );
    }
    #[test]
    fn blocks_right_aligned_with_separators() {
        let c = BarConfig::default().colors;
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[],
            true,
            None,
            &[blk("ab"), blk("cde")],
            &c,
            &m,
        );
        assert_eq!(l.items[1].rect, Rect::new(975, 0, 21, 22));
        assert_eq!(l.items[0].rect, Rect::new(952, 0, 14, 22));
        assert_eq!(l.separators, vec![970]);
        assert_eq!(l.items[1].fg, c.statusline);
    }
    #[test]
    fn status_never_overlaps_buttons() {
        let c = BarConfig::default().colors;
        let l = layout_bar(
            100,
            22,
            1.0,
            &[btn("1", true, true)],
            true,
            None,
            &[blk("aaaaaaaaaa"), blk("bbbbbb")],
            &c,
            &m,
        );
        assert_eq!(l.items.len(), 2);
        assert!(l.items.iter().all(|i| i.kind != ItemKind::Block(0)));
    }
    #[test]
    fn hit_testing() {
        let c = BarConfig::default().colors;
        let l = layout_bar(
            1000,
            22,
            1.0,
            &[btn("1", true, true)],
            true,
            None,
            &[],
            &c,
            &m,
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
                vec![Command::Workspace(WorkspaceTarget::Name(name.into()))]
            );
        }
    }
    #[test]
    fn odd_inputs_do_not_panic() {
        let c = BarConfig::default().colors;
        let mut b = blk("x");
        b.separator_block_width = Some(i32::MAX);
        let many: Vec<Block> = (0..10_000).map(|_| b.clone()).collect();
        for w in [0, -5, i32::MIN, i32::MAX] {
            let _ = layout_bar(
                w,
                22,
                1.0,
                &[btn("1", true, true)],
                true,
                Some("r"),
                &many,
                &c,
                &m,
            );
            let _ = layout_bar(
                w,
                22,
                1e9,
                &[btn("1", true, true)],
                true,
                None,
                &many,
                &c,
                &m,
            );
        }
        let _ = layout_bar(100, 22, 1.0, &[], true, None, &many, &c, &|_| i32::MAX);
    }
}
