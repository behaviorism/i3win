use crate::config::{
    BarPosition, Border, Config, Font, Gaps, HideEdgeBorders, SmartBorders, SmartGaps,
};
use crate::geom::Rect;
use crate::title::format_title;
use crate::tree::{Layout, NodeId, NodeKind, Tree, WindowId};
use crate::wm::Wm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecoState {
    Focused,
    FocusedInactive,
    Unfocused,
    Urgent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub title: String,
    pub state: DecoState,
    pub rect: Rect,
    pub node: NodeId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Deco {
    Frame {
        key: NodeId,
        rect: Rect,
        title_rect: Option<Rect>,
        /// left, top, right, bottom. With a normal (title) border top is 0 and the title bar sits there.
        borders: [i32; 4],
        title: String,
        state: DecoState,
        floating: bool,
        above: WindowId,
        /// Window whose icon to draw, and the padding in px.
        icon: Option<(WindowId, i32)>,
    },
    TabBar {
        key: NodeId,
        rect: Rect,
        vertical: bool,
        tabs: Vec<Tab>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowPlacement {
    pub id: WindowId,
    pub rect: Rect,
    pub floating: bool,
    pub fullscreen: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Arrangement {
    pub windows: Vec<WindowPlacement>,
    pub hidden: Vec<WindowId>,
    pub decos: Vec<Deco>,
    pub fullscreen_outputs: Vec<String>,
}

/// Upper bound for gaps and borders so geometry math cannot overflow.
const MAX_PX: i32 = 100_000;

fn px(v: i32) -> i32 {
    v.clamp(0, MAX_PX)
}

fn scaled(v: f32, scale: f32) -> i32 {
    // `as` saturates on overflow and maps NaN to 0.
    ((v * scale).round() as i32).clamp(0, MAX_PX)
}

pub fn title_height(font: &Font, scale: f32) -> i32 {
    scaled(font.size_pt * 96.0 / 72.0, scale) + scaled(8.0, scale)
}

pub fn bar_height(config: &Config, scale: f32) -> Option<i32> {
    let bar = config.bar.as_ref()?;
    Some(match bar.height {
        Some(h) => scaled(h as f32, scale),
        None => {
            let font = bar.font.as_ref().unwrap_or(&config.font);
            title_height(font, scale) + scaled(2.0, scale)
        }
    })
}

/// Which border lines the current workspace hides on tiled windows.
#[derive(Clone, Copy, Default)]
struct EdgeRules {
    vertical: bool,
    horizontal: bool,
    all: bool,
}

struct Ctx<'a> {
    tree: &'a Tree,
    config: &'a Config,
    machine: &'a str,
    focused: NodeId,
    th: i32,
    g: i32,
    /// Tiling area at frame level (after bar, outer gaps and half inner gap).
    area: Rect,
    edges: EdgeRules,
    out: Arrangement,
}

impl Ctx<'_> {
    fn has_urgent(&self, c: NodeId) -> bool {
        self.tree
            .windows_under(c)
            .into_iter()
            .any(|w| matches!(&self.tree.get(w).kind, NodeKind::Window(d) if d.urgent))
    }

    fn state(&self, c: NodeId) -> DecoState {
        let t = self.tree;
        if self.has_urgent(c) {
            return DecoState::Urgent;
        }
        let f = self.focused;
        if c == f || t.is_ancestor(c, f) || t.is_ancestor(f, c) {
            return DecoState::Focused;
        }
        match t.parent(c) {
            Some(p) if t.get(p).focus.first() == Some(&c) => DecoState::FocusedInactive,
            _ => DecoState::Unfocused,
        }
    }

    fn window_title(&self, node: NodeId) -> String {
        let n = self.tree.get(node);
        match &n.kind {
            NodeKind::Window(d) => format_title(
                n.title_format.as_deref(),
                &d.info.title,
                &d.info.class,
                &d.info.instance,
                self.machine,
                &n.marks,
                self.config.show_marks,
            ),
            _ => String::new(),
        }
    }

    fn hide_under(&mut self, c: NodeId) {
        for w in self.tree.windows_under(c) {
            if let Some(id) = self.tree.window_id(w) {
                self.out.hidden.push(id);
            }
        }
    }

    fn place(&mut self, node: NodeId, rect: Rect, in_tabs: bool) {
        let tree = self.tree;
        let n = tree.get(node);
        let th = self.th;
        match &n.kind {
            NodeKind::Window(_) => {
                let g = self.g;
                let frame = rect.inset(g / 2, g / 2, g - g / 2, g - g / 2);
                let border = n.border.unwrap_or(self.config.default_border);
                self.emit_window(node, frame, border, in_tabs, false);
            }
            NodeKind::Workspace { .. } | NodeKind::Container => {
                let count = n.children.len();
                if count == 0 {
                    return;
                }
                match n.layout {
                    Layout::SplitH | Layout::SplitV => {
                        let horizontal = n.layout == Layout::SplitH;
                        let total = if horizontal { rect.w } else { rect.h };
                        let mut used = 0i32;
                        for (i, &c) in n.children.iter().enumerate() {
                            let left = (total - used).max(0);
                            let size = if i + 1 < count {
                                let s = (tree.get(c).percent * total as f64).round();
                                s.clamp(0.0, left as f64) as i32
                            } else {
                                left
                            };
                            let r = if horizontal {
                                Rect::new(rect.x.saturating_add(used), rect.y, size, rect.h)
                            } else {
                                Rect::new(rect.x, rect.y.saturating_add(used), rect.w, size)
                            };
                            used = used.saturating_add(size);
                            self.place(c, r, false);
                        }
                    }
                    Layout::Tabbed | Layout::Stacked => {
                        let stacked = n.layout == Layout::Stacked;
                        let cnt = i32::try_from(count).unwrap_or(i32::MAX);
                        let header_h = if stacked { th.saturating_mul(cnt) } else { th };
                        let header = Rect::new(rect.x, rect.y, rect.w, header_h);
                        let mut tabs = Vec::with_capacity(count);
                        for (i, &c) in n.children.iter().enumerate() {
                            let idx = i32::try_from(i).unwrap_or(i32::MAX);
                            let r = if stacked {
                                let y = rect.y.saturating_add(idx.saturating_mul(th));
                                Rect::new(rect.x, y, rect.w, th)
                            } else {
                                let w = rect.w / cnt;
                                let before = w.saturating_mul(idx);
                                let tw = if i + 1 == count {
                                    (rect.w - before).max(0)
                                } else {
                                    w
                                };
                                Rect::new(rect.x.saturating_add(before), rect.y, tw, th)
                            };
                            tabs.push(Tab {
                                title: if matches!(tree.get(c).kind, NodeKind::Window(_)) {
                                    self.window_title(c)
                                } else {
                                    tree.describe(c)
                                },
                                state: self.state(c),
                                rect: r,
                                node: c,
                            });
                        }
                        self.out.decos.push(Deco::TabBar {
                            key: node,
                            rect: header,
                            vertical: stacked,
                            tabs,
                        });
                        let active = n.focus.iter().copied().find(|c| n.children.contains(c));
                        for &c in &n.children {
                            if Some(c) != active {
                                self.hide_under(c);
                            }
                        }
                        if let Some(a) = active {
                            self.place(a, rect.inset(0, header_h, 0, 0), true);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn emit_window(
        &mut self,
        node: NodeId,
        frame: Rect,
        border: Border,
        in_tabs: bool,
        floating: bool,
    ) {
        let Some(id) = self.tree.window_id(node) else {
            return;
        };
        let th = self.th;
        let width = match border {
            Border::Normal(b) | Border::Pixel(b) => px(b),
            Border::None => 0,
        };
        let titled = matches!(border, Border::Normal(_)) && !in_tabs;
        // The title bar (or the tab header above) takes the top: no top line.
        let mut sides = [width, width, width, width];
        if matches!(border, Border::Normal(_)) {
            sides[1] = 0;
        }
        if !floating {
            let e = self.edges;
            let a = self.area;
            if e.all || (e.vertical && frame.x == a.x) {
                sides[0] = 0;
            }
            if e.all || (e.vertical && frame.right() == a.right()) {
                sides[2] = 0;
            }
            if e.all || (e.horizontal && frame.y == a.y) {
                sides[1] = 0;
            }
            if e.all || (e.horizontal && frame.bottom() == a.bottom()) {
                sides[3] = 0;
            }
        }
        let top = if titled { th } else { sides[1] };
        let client = frame.inset(sides[0], top, sides[2], sides[3]);
        let title_rect = titled.then(|| Rect::new(frame.x, frame.y, frame.w, th));
        self.out.windows.push(WindowPlacement {
            id,
            rect: client,
            floating,
            fullscreen: false,
        });
        if border != Border::None && (titled || sides.iter().any(|&s| s > 0)) {
            let title = self.window_title(node);
            let state = self.state(node);
            self.out.decos.push(Deco::Frame {
                key: node,
                rect: frame,
                title_rect,
                borders: sides,
                title,
                state,
                floating,
                above: id,
                icon: self.tree.get(node).title_icon.map(|p| (id, p)),
            });
        }
    }
}

/// Config gaps, then the config `workspace NAME gaps` override, then the runtime override.
pub fn effective_gaps(tree: &Tree, config: &Config, ws: NodeId) -> Gaps {
    let node = tree.get(ws);
    let mut gaps = config.gaps;
    if let NodeKind::Workspace { name, .. } = &node.kind {
        if let Some((_, o)) = config.workspace_gaps.iter().find(|(n, _)| n == name) {
            gaps = gaps.with(o);
        }
    }
    gaps.with(&node.gaps)
}

/// Number of tiled windows shown in `id`'s subtree (only the active tab counts).
fn visible_tiled(tree: &Tree, id: NodeId) -> usize {
    let n = tree.get(id);
    if matches!(n.kind, NodeKind::Window(_)) {
        return 1;
    }
    if matches!(n.layout, Layout::Tabbed | Layout::Stacked) {
        let active = n.focus.iter().copied().find(|c| n.children.contains(c));
        return active.map_or(0, |a| visible_tiled(tree, a));
    }
    n.children.iter().map(|&c| visible_tiled(tree, c)).sum()
}

pub fn arrange(tree: &Tree, config: &Config, focused: NodeId, machine: &str) -> Arrangement {
    let mut ctx = Ctx {
        tree,
        config,
        machine,
        focused,
        th: 0,
        g: px(config.gaps.inner),
        area: Rect::default(),
        edges: EdgeRules::default(),
        out: Arrangement::default(),
    };
    for &o in &tree.get(tree.root()).children {
        let on = tree.get(o);
        let NodeKind::Output { name, rect, scale } = &on.kind else {
            continue;
        };
        let visible = on.focus.first().copied();
        for &ws in &on.children {
            if Some(ws) == visible {
                arrange_workspace(&mut ctx, config, ws, name, *rect, *scale);
            } else {
                ctx.hide_under(ws);
            }
        }
    }
    ctx.out
}

fn arrange_workspace(
    ctx: &mut Ctx,
    config: &Config,
    ws: NodeId,
    out_name: &str,
    out_rect: Rect,
    scale: f32,
) {
    let tree = ctx.tree;
    ctx.th = title_height(&config.font, scale);

    let single = visible_tiled(tree, ws) == 1;
    let mut gaps = effective_gaps(tree, config, ws);
    match config.smart_gaps {
        SmartGaps::On if single => gaps = Gaps::default(),
        SmartGaps::InverseOuter if !single => {
            gaps = Gaps {
                top: 0,
                right: 0,
                bottom: 0,
                left: 0,
                ..gaps
            }
        }
        _ => {}
    }
    let g = px(gaps.inner);
    ctx.g = g;
    let smart_all = |mode: HideEdgeBorders| match mode {
        HideEdgeBorders::Smart => single,
        HideEdgeBorders::SmartNoGaps => single && g == 0,
        _ => false,
    };
    let smart_cfg = match config.smart_borders {
        SmartBorders::Off => HideEdgeBorders::None,
        SmartBorders::On => HideEdgeBorders::Smart,
        SmartBorders::NoGaps => HideEdgeBorders::SmartNoGaps,
    };
    let he = config.hide_edge_borders;
    ctx.edges = EdgeRules {
        vertical: matches!(he, HideEdgeBorders::Vertical | HideEdgeBorders::Both),
        horizontal: matches!(he, HideEdgeBorders::Horizontal | HideEdgeBorders::Both),
        all: smart_all(he) || smart_all(smart_cfg),
    };

    // Fullscreen: one window covers the whole output, everything else hides.
    let all = tree.windows_under(ws);
    if let Some(&fs) = all.iter().find(|&&w| tree.get(w).fullscreen) {
        if let Some(id) = tree.window_id(fs) {
            ctx.out.windows.push(WindowPlacement {
                id,
                rect: out_rect,
                floating: tree.is_floating(fs),
                fullscreen: true,
            });
            ctx.out.fullscreen_outputs.push(out_name.to_string());
            let floating = &tree.get(ws).floating;
            for &w in all.iter().filter(|&&w| w != fs && !floating.contains(&w)) {
                if let Some(wid) = tree.window_id(w) {
                    ctx.out.hidden.push(wid);
                }
            }
            // Floating windows (dialogs) stay on top of the fullscreen window.
            place_floating(ctx, config, ws, Some(fs));
            return;
        }
    }

    let mut area = out_rect;
    if let Some(bh) = bar_height(config, scale) {
        let top = config.bar.as_ref().map(|b| b.position) == Some(BarPosition::Top);
        area = if top {
            area.inset(0, bh, 0, 0)
        } else {
            area.inset(0, 0, 0, bh)
        };
    }
    let lead = |outer: i32| px(outer) + g - g / 2;
    let trail = |outer: i32| px(outer) + g / 2;
    let inset = area.inset(
        lead(gaps.left),
        lead(gaps.top),
        trail(gaps.right),
        trail(gaps.bottom),
    );
    ctx.area = inset.inset(g / 2, g / 2, g - g / 2, g - g / 2);
    ctx.place(ws, inset, false);

    place_floating(ctx, config, ws, None);
}

/// Places the floating windows of `ws` (skipping `skip`) above the tiled ones.
fn place_floating(ctx: &mut Ctx, config: &Config, ws: NodeId, skip: Option<NodeId>) {
    let tree = ctx.tree;
    for &f in &tree.get(ws).floating {
        if Some(f) == skip {
            continue;
        }
        let NodeKind::Window(d) = &tree.get(f).kind else {
            continue;
        };
        let frame = d.floating_rect.unwrap_or(d.info.rect);
        let border = tree.get(f).border.unwrap_or(config.default_floating_border);
        ctx.emit_window(f, frame, border, false, true);
    }
}

/// Inverse of the floating frame -> client inset used by `arrange`.
pub fn floating_frame_from_client(config: &Config, scale: f32, client: Rect) -> Rect {
    let th = title_height(&config.font, scale);
    let grow = |l: i32, t: i32, r: i32, b: i32| {
        Rect::new(
            client.x.saturating_sub(l),
            client.y.saturating_sub(t),
            client.w.saturating_add(l + r),
            client.h.saturating_add(t + b),
        )
    };
    match config.default_floating_border {
        Border::Normal(b) => {
            let b = px(b);
            grow(b, th, b, b)
        }
        Border::Pixel(b) => {
            let b = px(b);
            grow(b, b, b, b)
        }
        Border::None => client,
    }
}

impl Wm {
    pub fn effective_gaps(&self, ws: NodeId) -> Gaps {
        effective_gaps(&self.tree, &self.config, ws)
    }

    pub fn arrange(&self) -> Arrangement {
        arrange(&self.tree, &self.config, self.focused(), &self.machine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{parse_config, BarConfig, Border, Font};
    use crate::testutil::{out, test_wm, win};

    fn placement(a: &Arrangement, w: isize) -> Rect {
        a.windows.iter().find(|p| p.id == WindowId(w)).unwrap().rect
    }
    #[test]
    fn heights() {
        let f = Font {
            family: "x".into(),
            size_pt: 9.0,
        };
        assert_eq!(title_height(&f, 1.0), 20);
        assert_eq!(title_height(&f, 1.5), 30);
    }
    #[test]
    fn splith_two_windows() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let a = wm.arrange();
        assert_eq!(placement(&a, 1), Rect::new(2, 20, 496, 478));
        assert_eq!(placement(&a, 2), Rect::new(502, 20, 496, 478));
        assert!(a.decos.contains(&Deco::Frame {
            key: wm.tree.window_node(WindowId(1)).unwrap(),
            rect: Rect::new(0, 0, 500, 500),
            title_rect: Some(Rect::new(0, 0, 500, 20)),
            borders: [2, 0, 2, 2],
            title: "w1".into(),
            state: DecoState::Unfocused,
            floating: false,
            above: WindowId(1),
            icon: None,
        }));
    }
    #[test]
    fn rounding_gives_remainder_to_last() {
        let mut wm = test_wm();
        wm.config.default_border = Border::None;
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        let a = wm.arrange();
        assert_eq!(
            (placement(&a, 1).w, placement(&a, 2).w, placement(&a, 3).w),
            (333, 333, 334)
        );
    }
    #[test]
    fn tabbed_layout() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("layout tabbed");
        let a = wm.arrange();
        assert_eq!(placement(&a, 2), Rect::new(2, 20, 996, 478));
        assert_eq!(a.hidden, vec![WindowId(1)]);
        let tb = a
            .decos
            .iter()
            .find_map(|d| match d {
                Deco::TabBar {
                    rect,
                    tabs,
                    vertical,
                    ..
                } => Some((*rect, tabs.clone(), *vertical)),
                _ => None,
            })
            .unwrap();
        assert_eq!(tb.0, Rect::new(0, 0, 1000, 20));
        assert!(!tb.2);
        assert_eq!(
            tb.1.iter()
                .map(|t| (t.title.as_str(), t.rect, t.state))
                .collect::<Vec<_>>(),
            vec![
                ("w1", Rect::new(0, 0, 500, 20), DecoState::Unfocused),
                ("w2", Rect::new(500, 0, 500, 20), DecoState::Focused)
            ]
        );
    }
    #[test]
    fn stacked_layout() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("layout stacking");
        let a = wm.arrange();
        assert_eq!(placement(&a, 2), Rect::new(2, 40, 996, 458));
    }
    #[test]
    fn container_tab_title_uses_describe() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("split v");
        wm.manage(win(3));
        wm.run_command("focus parent");
        wm.run_command("focus left");
        wm.run_command("layout tabbed");
        let a = wm.arrange();
        let titles: Vec<String> = a
            .decos
            .iter()
            .find_map(|d| match d {
                Deco::TabBar { tabs, .. } => Some(tabs.iter().map(|t| t.title.clone()).collect()),
                _ => None,
            })
            .unwrap();
        assert_eq!(titles, vec!["w1".to_string(), "V[w2 w3]".to_string()]);
    }
    #[test]
    fn gaps() {
        let mut wm = test_wm();
        wm.config.default_border = Border::None;
        wm.config.gaps = Gaps {
            inner: 10,
            top: 5,
            right: 5,
            bottom: 5,
            left: 5,
        };
        wm.manage(win(1));
        wm.manage(win(2));
        let a = wm.arrange();
        assert_eq!(placement(&a, 1), Rect::new(15, 15, 480, 470));
        assert_eq!(placement(&a, 2), Rect::new(505, 15, 480, 470));
    }
    #[test]
    fn per_side_outer_gaps_and_workspace_override() {
        let mut cfg = crate::config::parse_config(
            "gaps top 20\ngaps left 10\nworkspace 2 gaps top 0\ndefault_border none\n",
        );
        cfg.bar = None;
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        let r = wm.arrange().windows[0].rect;
        assert_eq!((r.x, r.y, r.w, r.h), (10, 20, 990, 480));
        wm.run_command("workspace 2");
        wm.manage(win(2));
        let r2 = wm
            .arrange()
            .windows
            .iter()
            .find(|p| p.id == WindowId(2))
            .unwrap()
            .rect;
        assert_eq!((r2.x, r2.y), (10, 0));
    }
    #[test]
    fn bar_reserves_space() {
        let mut wm = test_wm();
        wm.config.bar = Some(BarConfig::default());
        wm.config.default_border = Border::None;
        wm.manage(win(1));
        assert_eq!(bar_height(&wm.config, 1.0), Some(22));
        assert_eq!(placement(&wm.arrange(), 1), Rect::new(0, 0, 1000, 478));
    }
    #[test]
    fn fullscreen_covers_output_and_hides_rest() {
        let mut wm = test_wm();
        wm.config.bar = Some(BarConfig::default());
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("fullscreen enable");
        let a = wm.arrange();
        assert_eq!(
            a.windows,
            vec![WindowPlacement {
                id: WindowId(2),
                rect: Rect::new(0, 0, 1000, 500),
                floating: false,
                fullscreen: true
            }]
        );
        assert_eq!(a.hidden, vec![WindowId(1)]);
        assert_eq!(a.fullscreen_outputs, vec!["O1".to_string()]);
        assert!(a.decos.is_empty());
    }
    #[test]
    fn hidden_workspace_windows_are_hidden() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("workspace 2");
        let a = wm.arrange();
        assert!(a.windows.is_empty());
        assert_eq!(a.hidden, vec![WindowId(1)]);
    }
    #[test]
    fn floating_frame_and_client() {
        let mut wm = test_wm();
        let mut f = win(1);
        f.wants_float = true;
        wm.manage(f);
        let a = wm.arrange();
        assert_eq!(placement(&a, 1), Rect::new(302, 120, 396, 278));
        assert!(a.windows[0].floating);
        assert_eq!(
            floating_frame_from_client(&wm.config, 1.0, Rect::new(302, 120, 396, 278)),
            Rect::new(300, 100, 400, 300)
        );
    }
    #[test]
    fn focused_states() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        let a = wm.arrange();
        let st = |w: isize| {
            a.decos
                .iter()
                .find_map(|d| match d {
                    Deco::Frame { above, state, .. } if *above == WindowId(w) => Some(*state),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(st(2), DecoState::Focused);
        assert_eq!(st(1), DecoState::Unfocused);
    }
    #[test]
    fn fullscreen_keeps_floating_dialog_on_top() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.manage(win(3));
        wm.run_command("floating enable");
        wm.run_command("[class=\"c1\"] fullscreen enable");
        let a = wm.arrange();
        let pos = |w: isize| a.windows.iter().position(|p| p.id == WindowId(w));
        assert!(pos(1).is_some() && pos(3).is_some());
        assert!(pos(3) > pos(1), "dialog placed after fullscreen window");
        assert!(a.hidden.contains(&WindowId(2)));
        assert!(!a.hidden.contains(&WindowId(3)));
        assert!(a.windows[pos(3).unwrap()].floating);
    }
    #[test]
    fn border_command_and_toggle() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("border pixel 3");
        let Deco::Frame {
            borders,
            title_rect,
            ..
        } = &wm.arrange().decos[0]
        else {
            panic!()
        };
        assert_eq!(*borders, [3, 3, 3, 3]);
        assert!(title_rect.is_none());
        wm.run_command("border toggle");
        assert!(wm.arrange().decos.is_empty()); // pixel -> none
        wm.run_command("border toggle");
        assert!(matches!(
            &wm.arrange().decos[0],
            Deco::Frame {
                title_rect: Some(_),
                ..
            }
        ));
    }
    #[test]
    fn title_format_applies_to_frame_title() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("title_format \"<%class> %title\"");
        let Deco::Frame { title, .. } = &wm.arrange().decos[0] else {
            panic!()
        };
        assert_eq!(title, "<c1> w1");
    }
    #[test]
    fn title_format_applies_to_tab_titles() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("layout tabbed");
        wm.run_command("title_format \"%instance\"");
        let a = wm.arrange();
        let tabs = a
            .decos
            .iter()
            .find_map(|d| match d {
                Deco::TabBar { tabs, .. } => Some(tabs),
                _ => None,
            })
            .unwrap();
        assert!(tabs.iter().any(|t| t.title == "i2"));
        assert!(tabs.iter().any(|t| t.title == "w1"));
    }
    #[test]
    fn hidden_edge_borders_shift_client_rect_exactly() {
        let mut cfg = parse_config(
            "hide_edge_borders both
default_border pixel 2
",
        );
        cfg.bar = None;
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        wm.manage(win(2));
        let a = wm.arrange();
        let w1 = a.windows.iter().find(|p| p.id == WindowId(1)).unwrap().rect;
        let w2 = a.windows.iter().find(|p| p.id == WindowId(2)).unwrap().rect;
        assert_eq!((w1.x, w1.y, w1.h), (0, 0, 500)); // outer sides hidden
        assert_eq!(w1.right() + 2 + 2, w2.x); // inner sides keep 2 px each
        assert_eq!(w2.right(), 1000);
    }
    #[test]
    fn smart_borders_only_with_single_window() {
        let mut cfg = parse_config(
            "smart_borders on
",
        );
        cfg.bar = None;
        let mut wm = Wm::new(cfg, vec![out("O1", 0, 1000)]);
        wm.manage(win(1));
        let Deco::Frame { borders, .. } = &wm.arrange().decos[0] else {
            panic!()
        };
        assert_eq!(*borders, [0, 0, 0, 0]);
        wm.manage(win(2));
        let Deco::Frame {
            borders,
            title_rect,
            ..
        } = &wm.arrange().decos[0]
        else {
            panic!()
        };
        assert_eq!(*borders, [2, 0, 2, 2]);
        assert!(title_rect.is_some());
    }
    #[test]
    fn smart_gaps_single_window_has_no_gaps() {
        let mut cfg = parse_config(
            "gaps inner 10
gaps outer 5
smart_gaps on
default_border none
",
        );
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
        let Deco::Frame { icon, .. } = &wm.arrange().decos[0] else {
            panic!()
        };
        assert_eq!(*icon, Some((WindowId(1), 4)));
    }
}
