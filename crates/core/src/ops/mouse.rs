use crate::geom::{Direction, Rect, MIN_FLOAT_SIZE};
use crate::tree::{Layout, NodeId, NodeKind};
use crate::wm::Wm;

const MIN_PERCENT: f64 = 0.05;

/// Floating window corner grabbed by a resize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Which horizontal and vertical edge a press at (x, y) inside `r` grabs:
/// left/right half, top/bottom half.
pub fn edges_for_press(r: Rect, x: i32, y: i32) -> (Direction, Direction) {
    let h = if i64::from(x) < i64::from(r.x) + i64::from(r.w) / 2 {
        Direction::Left
    } else {
        Direction::Right
    };
    let v = if i64::from(y) < i64::from(r.y) + i64::from(r.h) / 2 {
        Direction::Up
    } else {
        Direction::Down
    };
    (h, v)
}

pub fn corner_for_press(r: Rect, x: i32, y: i32) -> Corner {
    match edges_for_press(r, x, y) {
        (Direction::Left, Direction::Up) => Corner::TopLeft,
        (Direction::Right, Direction::Up) => Corner::TopRight,
        (Direction::Left, _) => Corner::BottomLeft,
        _ => Corner::BottomRight,
    }
}

/// Floating corner resize: the opposite corner stays fixed; size clamped to
/// [min, max] (max 0 = unlimited) and MIN_FLOAT_SIZE.
pub fn resize_from_corner(
    start: Rect,
    corner: Corner,
    dx: i32,
    dy: i32,
    min: (i32, i32),
    max: (i32, i32),
) -> Rect {
    let left = matches!(corner, Corner::TopLeft | Corner::BottomLeft);
    let top = matches!(corner, Corner::TopLeft | Corner::TopRight);
    let clamp = |v: i64, lo: i32, hi: i32| {
        let v = v.min(if hi > 0 { i64::from(hi) } else { i64::MAX });
        v.max(i64::from(lo.max(MIN_FLOAT_SIZE))) as i32
    };
    let dw = if left { -i64::from(dx) } else { i64::from(dx) };
    let dh = if top { -i64::from(dy) } else { i64::from(dy) };
    let w = clamp(i64::from(start.w) + dw, min.0, max.0);
    let h = clamp(i64::from(start.h) + dh, min.1, max.1);
    let x = if left {
        start.x.saturating_add(start.w).saturating_sub(w)
    } else {
        start.x
    };
    let y = if top {
        start.y.saturating_add(start.h).saturating_sub(h)
    } else {
        start.y
    };
    Rect::new(x, y, w, h)
}

/// Where on a tiled window a dragged window is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Swap,
    Side(Direction),
}

/// Center box (middle 30% of width and height) -> Swap; else the triangle
/// formed by the rect's diagonals that contains the point.
pub fn drop_zone(r: Rect, x: i32, y: i32) -> DropZone {
    let (w, h) = (i64::from(r.w), i64::from(r.h));
    // Offset from the center, doubled to stay in integers.
    let dx = 2 * i64::from(x) - 2 * i64::from(r.x) - w;
    let dy = 2 * i64::from(y) - 2 * i64::from(r.y) - h;
    // |dx| <= 0.15 * w (offsets are doubled: 2 * 0.15 = 0.3).
    if dx.abs() * 10 <= 3 * w && dy.abs() * 10 <= 3 * h {
        return DropZone::Swap;
    }
    // Compare against the diagonals: |dx|/w vs |dy|/h.
    if dx.abs() * h >= dy.abs() * w {
        DropZone::Side(if dx < 0 {
            Direction::Left
        } else {
            Direction::Right
        })
    } else {
        DropZone::Side(if dy < 0 {
            Direction::Up
        } else {
            Direction::Down
        })
    }
}

/// What a tiling drag would do if released at a point.
#[derive(Clone, Debug, PartialEq)]
pub enum DropTarget {
    Window(NodeId, DropZone),
    Workspace(String),
    Output(NodeId),
    Nothing,
}

fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

fn split_layout(d: Direction) -> Layout {
    match d {
        Direction::Left | Direction::Right => Layout::SplitH,
        Direction::Up | Direction::Down => Layout::SplitV,
    }
}

impl Wm {
    /// Whether `target` is a tiled, non-fullscreen window other than `dragged`
    /// that `dragged` may be dropped onto.
    fn is_drop_window(&self, dragged: NodeId, target: NodeId) -> bool {
        self.tree.contains(dragged)
            && self.tree.contains(target)
            && dragged != target
            && matches!(self.tree.get(target).kind, NodeKind::Window(_))
            && !self.tree.get(target).fullscreen
            && !self.tree.is_floating(target)
            && !self.tree.is_ancestor(dragged, target)
            && self
                .tree
                .workspace_of(target)
                .is_some_and(|w| w != self.scratch)
    }

    /// Output rect, or the visible workspace's tiling area when it has one.
    fn output_area(&self, output: NodeId) -> Option<Rect> {
        self.node_rect(self.visible_workspace(output)).or_else(|| {
            match &self.tree.get(output).kind {
                NodeKind::Output { rect, .. } => Some(*rect),
                _ => None,
            }
        })
    }

    /// Target under a screen point for a tiling drag of `dragged` (bar hits
    /// are resolved by the platform into Workspace).
    pub fn drop_target_at(&self, dragged: NodeId, x: i32, y: i32) -> DropTarget {
        let outputs = self.outputs();
        let arrangement = self.arrange();
        let hit = arrangement.rects.iter().find(|(n, r)| {
            contains(*r, x, y)
                && matches!(self.tree.get(*n).kind, NodeKind::Window(_))
                && !self.tree.is_floating(*n)
                && self
                    .tree
                    .output_of(*n)
                    .is_some_and(|o| outputs.contains(&o))
        });
        if let Some(&(n, r)) = hit {
            if !self.is_drop_window(dragged, n) {
                return DropTarget::Nothing;
            }
            return DropTarget::Window(n, drop_zone(r, x, y));
        }
        for o in outputs {
            if let NodeKind::Output { rect, .. } = &self.tree.get(o).kind {
                if contains(*rect, x, y) {
                    return DropTarget::Output(o);
                }
            }
        }
        DropTarget::Nothing
    }

    /// Performs the drop; returns whether anything changed. Focus stays on
    /// `dragged`.
    pub fn drop_window(&mut self, dragged: NodeId, target: &DropTarget) -> bool {
        if !self.tree.contains(dragged)
            || !matches!(self.tree.get(dragged).kind, NodeKind::Window(_))
            || self.tree.is_floating(dragged)
        {
            return false;
        }
        let Some(src) = self.tree.workspace_of(dragged) else {
            return false;
        };
        match target {
            DropTarget::Nothing => false,
            DropTarget::Window(t, zone) => {
                if !self.is_drop_window(dragged, *t) {
                    return false;
                }
                let Some(dst) = self.tree.workspace_of(*t) else {
                    return false;
                };
                let in_tabs = self.tree.parent(*t).is_some_and(|p| {
                    matches!(self.tree.get(p).layout, Layout::Tabbed | Layout::Stacked)
                });
                let mut moved_windows = vec![dragged];
                match zone {
                    DropZone::Swap if !in_tabs => {
                        self.tree.swap(dragged, *t);
                        if self.tree.workspace_of(dragged) != Some(dst) {
                            return false;
                        }
                        if src != dst {
                            self.on_node_rehomed(dragged, Some(src), dst);
                            self.on_node_rehomed(*t, Some(dst), src);
                        }
                        moved_windows.push(*t);
                    }
                    _ => {
                        let d = match zone {
                            DropZone::Side(d) => *d,
                            DropZone::Swap => Direction::Right,
                        };
                        self.insert_beside(dragged, *t, d);
                        if src != dst {
                            self.on_node_rehomed(dragged, Some(src), dst);
                        }
                    }
                }
                self.finish_drop(dragged, src);
                for n in moved_windows {
                    if let Some(w) = self.tree.window_id(n) {
                        self.emit_window("move", w);
                    }
                }
                true
            }
            DropTarget::Workspace(name) => {
                if name.starts_with("__") {
                    return false;
                }
                let dest = self.find_or_create_workspace(name);
                self.move_dragged(dragged, dest)
            }
            DropTarget::Output(o) => {
                if !self.outputs().contains(o) {
                    return false;
                }
                let dest = self.visible_workspace(*o);
                self.move_dragged(dragged, dest)
            }
        }
    }

    /// Detaches `dragged` (with v1's removal cleanup) and inserts it next to
    /// `t` according to `d`, or as the next tab when `t` sits in tabs.
    fn insert_beside(&mut self, dragged: NodeId, t: NodeId, d: Direction) {
        let old_parent = self.tree.parent(dragged);
        self.tree.detach(dragged);
        if let Some(p) = old_parent {
            if matches!(self.tree.get(p).kind, NodeKind::Container) {
                self.tree.cleanup(p);
            }
        }
        let (Some(parent), Some(idx)) = (self.tree.parent(t), self.tree.index_in_parent(t)) else {
            return;
        };
        let layout = self.tree.get(parent).layout;
        if matches!(layout, Layout::Tabbed | Layout::Stacked) {
            self.tree.attach(parent, dragged, Some(idx + 1));
            return;
        }
        let after = matches!(d, Direction::Right | Direction::Down);
        if layout == split_layout(d) {
            self.tree
                .attach(parent, dragged, Some(if after { idx + 1 } else { idx }));
        } else {
            let c = self.tree.wrap(t, split_layout(d));
            self.tree
                .attach(c, dragged, if after { None } else { Some(0) });
        }
    }

    /// Focus `dragged`, and drop the source workspace if it was left empty
    /// and hidden.
    fn finish_drop(&mut self, dragged: NodeId, src: NodeId) {
        self.focused_container = None;
        let visible = self
            .tree
            .workspace_of(dragged)
            .is_some_and(|w| self.is_visible(w));
        if visible {
            self.tree.set_focused(dragged);
        } else {
            self.focus_in_place(dragged);
        }
        self.remove_if_empty_hidden(src);
    }

    /// Moves `dragged` to the end of `dest`. Focus stays on it when `dest` is
    /// visible (another output); a hidden `dest` is not followed.
    fn move_dragged(&mut self, dragged: NodeId, dest: NodeId) -> bool {
        if self.tree.workspace_of(dragged) == Some(dest) {
            return false;
        }
        let moved = self.move_node_to_workspace(dragged, dest, true).is_ok()
            && self.tree.workspace_of(dragged) == Some(dest);
        if moved && self.is_visible(dest) {
            self.focus_node(dragged);
        }
        moved
    }

    /// Rect to preview for a target (the zone's half/whole target rect, or
    /// the workspace area).
    pub fn drop_preview_rect(&self, target: &DropTarget) -> Option<Rect> {
        match target {
            DropTarget::Nothing => None,
            DropTarget::Output(o) => self.output_area(*o),
            DropTarget::Workspace(name) => {
                let o = self
                    .find_workspace(name)
                    .and_then(|w| self.tree.output_of(w))
                    .unwrap_or_else(|| self.focused_output());
                self.output_area(o)
            }
            DropTarget::Window(t, zone) => {
                let r = self.node_rect(*t)?;
                let in_tabs = self.tree.parent(*t).is_some_and(|p| {
                    matches!(self.tree.get(p).layout, Layout::Tabbed | Layout::Stacked)
                });
                Some(match zone {
                    DropZone::Side(d) if !in_tabs => match d {
                        Direction::Left => Rect::new(r.x, r.y, r.w / 2, r.h),
                        Direction::Right => Rect::new(r.x + r.w / 2, r.y, r.w - r.w / 2, r.h),
                        Direction::Up => Rect::new(r.x, r.y, r.w, r.h / 2),
                        Direction::Down => Rect::new(r.x, r.y + r.h / 2, r.w, r.h - r.h / 2),
                    },
                    _ => r,
                })
            }
        }
    }

    /// Rect of `node` in the current arrangement (tiled windows and
    /// containers: the layout rect before gaps/borders).
    pub fn node_rect(&self, node: NodeId) -> Option<Rect> {
        self.arrange()
            .rects
            .into_iter()
            .find(|(n, _)| *n == node)
            .map(|(_, r)| r)
    }

    /// i3 directional resize: moves the `dir` edge of `node` by `delta_px`
    /// (positive = outward). Walks up to the nearest splith (Left/Right) or
    /// splitv (Up/Down) ancestor where the walked-up child has a sibling on
    /// that side, and moves the boundary between them, keeping both >= 5%.
    /// Returns false when no such ancestor exists.
    pub fn resize_edge(&mut self, node: NodeId, dir: Direction, delta_px: i32) -> bool {
        let (want, forward) = match dir {
            Direction::Left => (Layout::SplitH, false),
            Direction::Right => (Layout::SplitH, true),
            Direction::Up => (Layout::SplitV, false),
            Direction::Down => (Layout::SplitV, true),
        };
        let mut c = node;
        let found = loop {
            let Some(p) = self.tree.parent(c) else {
                return false;
            };
            let pn = self.tree.get(p);
            if !matches!(pn.kind, NodeKind::Workspace { .. } | NodeKind::Container) {
                return false;
            }
            if pn.layout == want {
                if let Some(i) = pn.children.iter().position(|&s| s == c) {
                    let sib = if forward {
                        pn.children.get(i + 1)
                    } else {
                        i.checked_sub(1).and_then(|j| pn.children.get(j))
                    };
                    if let Some(&s) = sib {
                        break (p, s);
                    }
                }
            }
            c = p;
        };
        let (p, s) = found;
        let Some(pr) = self.node_rect(p) else {
            return false;
        };
        let total = if want == Layout::SplitH { pr.w } else { pr.h };
        if total <= 0 {
            return false;
        }
        let (pc, ps) = (self.tree.get(c).percent, self.tree.get(s).percent);
        let want_delta = f64::from(delta_px) / f64::from(total);
        let lo = (MIN_PERCENT - pc).min(0.0);
        let hi = (ps - MIN_PERCENT).max(0.0);
        let delta = want_delta.clamp(lo, hi);
        if !delta.is_finite() || delta.abs() < 1e-12 {
            return false;
        }
        self.tree.get_mut(c).percent = pc + delta;
        self.tree.get_mut(s).percent = ps - delta;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use crate::tree::WindowId;

    #[test]
    fn resize_edge_moves_boundary() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        wm.manage(win(1));
        wm.manage(win(2));
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        assert!(wm.resize_edge(n1, Direction::Right, 100));
        assert!((wm.tree.get(n1).percent - 0.6).abs() < 1e-9);
        assert!(wm.resize_edge(n2, Direction::Left, 100));
        assert!((wm.tree.get(n2).percent - 0.5).abs() < 1e-9);
        assert!(!wm.resize_edge(n1, Direction::Left, 50));
        assert!(!wm.resize_edge(n1, Direction::Up, 50));
        assert!(wm.resize_edge(n1, Direction::Right, 10_000));
        assert!((wm.tree.get(n2).percent - 0.05).abs() < 1e-9);
    }

    #[test]
    fn resize_edge_skips_tabbed_and_nested() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("split v");
        wm.manage(win(3));
        let n3 = wm.tree.window_node(WindowId(3)).unwrap();
        assert!(wm.resize_edge(n3, Direction::Left, 100));
        let v = wm.tree.parent(n3).unwrap();
        assert!((wm.tree.get(v).percent - 0.6).abs() < 1e-9);
        assert!(wm.resize_edge(n3, Direction::Up, 50));
    }

    #[test]
    fn resize_edge_many_tiles_never_panics_or_grows_wrong_way() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        for i in 1..=25 {
            wm.manage(win(i));
        }
        let ws = wm.focused_workspace();
        let n = |i: isize, wm: &Wm| wm.tree.window_node(WindowId(i)).unwrap();
        let (a, b) = (n(3, &wm), n(4, &wm));
        let pa = wm.tree.get(a).percent;
        assert!(pa < 0.05);
        let _ = wm.resize_edge(a, Direction::Right, -30);
        assert!(wm.tree.get(a).percent <= pa + 1e-12);
        let _ = wm.resize_edge(a, Direction::Right, 30);
        let _ = wm.resize_edge(b, Direction::Left, 30);
        let _ = wm.resize_edge(b, Direction::Left, -30);
        let sum: f64 = wm
            .tree
            .get(ws)
            .children
            .iter()
            .map(|&c| wm.tree.get(c).percent)
            .sum();
        assert!((sum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn zones() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(drop_zone(r, 50, 50), DropZone::Swap);
        assert_eq!(drop_zone(r, 5, 50), DropZone::Side(Direction::Left));
        assert_eq!(drop_zone(r, 50, 95), DropZone::Side(Direction::Down));
        assert_eq!(drop_zone(r, 10, 5), DropZone::Side(Direction::Up));
        assert_eq!(drop_zone(r, 95, 30), DropZone::Side(Direction::Right));
    }

    #[test]
    fn drop_sides_and_swap() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        for i in 1..=3 {
            wm.manage(win(i));
        }
        let n = |wm: &Wm, i| wm.tree.window_node(WindowId(i)).unwrap();
        assert!(wm.drop_window(
            n(&wm, 3),
            &DropTarget::Window(n(&wm, 1), DropZone::Side(Direction::Left))
        ));
        assert_eq!(ws_desc(&wm), "H[w3 w1 w2]");
        assert!(wm.drop_window(n(&wm, 3), &DropTarget::Window(n(&wm, 2), DropZone::Swap)));
        assert_eq!(ws_desc(&wm), "H[w2 w1 w3]");
        assert!(wm.drop_window(
            n(&wm, 3),
            &DropTarget::Window(n(&wm, 1), DropZone::Side(Direction::Down))
        ));
        assert_eq!(ws_desc(&wm), "H[w2 V[w1 w3]]");
        assert_eq!(wm.focused_window(), Some(WindowId(3)));
        check_invariants(&wm);
    }

    #[test]
    fn drop_into_tabbed_and_workspace() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        for i in 1..=3 {
            wm.manage(win(i));
        }
        let n = |wm: &Wm, i| wm.tree.window_node(WindowId(i)).unwrap();
        wm.run_command("[id=1] split v, layout tabbed");
        assert!(wm.drop_window(
            n(&wm, 3),
            &DropTarget::Window(n(&wm, 1), DropZone::Side(Direction::Right))
        ));
        assert_eq!(ws_desc(&wm), "H[T[w1 w3] w2]");
        assert!(wm.drop_window(n(&wm, 2), &DropTarget::Workspace("4".into())));
        assert_eq!(wm.tree.describe(wm.find_workspace("4").unwrap()), "H[w2]");
        check_invariants(&wm);
    }

    #[test]
    fn drop_swap_on_tab_inserts_tab() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        for i in 1..=3 {
            wm.manage(win(i));
        }
        let n = |wm: &Wm, i| wm.tree.window_node(WindowId(i)).unwrap();
        wm.run_command("[id=1] split v, layout tabbed");
        assert!(wm.drop_window(n(&wm, 3), &DropTarget::Window(n(&wm, 1), DropZone::Swap)));
        assert_eq!(ws_desc(&wm), "H[T[w1 w3] w2]");
        check_invariants(&wm);
    }

    #[test]
    fn drop_side_across_outputs() {
        let mut wm = Wm::new(
            crate::config::Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.config.bars.clear();
        wm.manage(win(1));
        wm.manage(win(2));
        wm.run_command("focus output right");
        wm.manage(win(3));
        wm.run_command("[id=2] fullscreen enable");
        let n = |wm: &Wm, i| wm.tree.window_node(WindowId(i)).unwrap();
        assert!(wm.drop_window(
            n(&wm, 2),
            &DropTarget::Window(n(&wm, 3), DropZone::Side(Direction::Down))
        ));
        let b = wm.outputs()[1];
        assert_eq!(wm.tree.describe(wm.visible_workspace(b)), "H[V[w3 w2]]");
        assert!(!wm.tree.get(n(&wm, 2)).fullscreen);
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
        check_invariants(&wm);
    }

    #[test]
    fn drop_noop_cases() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        wm.manage(win(1));
        wm.manage(win(2));
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        assert!(!wm.drop_window(n1, &DropTarget::Window(n1, DropZone::Swap)));
        assert!(!wm.drop_window(n1, &DropTarget::Nothing));
        wm.run_command("[id=2] floating enable");
        let c = wm.node_rect(n1).unwrap().center();
        assert_eq!(wm.drop_target_at(n1, c.0, c.1), DropTarget::Nothing);
        assert_eq!(
            wm.drop_target_at(n2, c.0, c.1),
            DropTarget::Window(n1, DropZone::Swap)
        );
    }

    #[test]
    fn drop_output_and_preview() {
        let mut wm = Wm::new(
            crate::config::Config::default(),
            vec![out("A", 0, 1000), out("B", 1000, 1000)],
        );
        wm.config.bars.clear();
        wm.manage(win(1));
        wm.manage(win(2));
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        let b = wm.outputs()[1];
        assert_eq!(wm.drop_target_at(n2, 1500, 100), DropTarget::Output(b));
        let area = wm.drop_preview_rect(&DropTarget::Output(b)).unwrap();
        assert!(area.x >= 1000);
        assert!(wm.drop_window(n2, &DropTarget::Output(b)));
        assert_eq!(wm.tree.describe(wm.visible_workspace(b)), "H[w2]");
        // The destination is visible: focus stays on the dragged window.
        assert_eq!(wm.focused_window(), Some(WindowId(2)));
        assert_eq!(wm.focused_workspace(), wm.visible_workspace(b));
        assert!(!wm.drop_window(n2, &DropTarget::Output(b)));
        let r = wm.node_rect(n1).unwrap();
        let left = wm
            .drop_preview_rect(&DropTarget::Window(n1, DropZone::Side(Direction::Left)))
            .unwrap();
        assert_eq!(left, Rect::new(r.x, r.y, r.w / 2, r.h));
        // Cross-output swap with a window on the other output.
        wm.manage(win(3));
        let n3 = wm.tree.window_node(WindowId(3)).unwrap();
        let ws_b = wm.visible_workspace(b);
        let t = wm.tree.get(ws_b).children[0];
        assert!(wm.drop_window(n3, &DropTarget::Window(t, DropZone::Swap)));
        check_invariants(&wm);
    }

    #[test]
    fn press_edges_and_corner_resize() {
        let r = Rect::new(0, 0, 100, 100);
        assert_eq!(
            edges_for_press(r, 10, 90),
            (Direction::Left, Direction::Down)
        );
        assert_eq!(corner_for_press(r, 90, 10), Corner::TopRight);
        let s = Rect::new(100, 100, 200, 200);
        assert_eq!(
            resize_from_corner(s, Corner::BottomRight, 50, 20, (75, 50), (0, 0)),
            Rect::new(100, 100, 250, 220)
        );
        assert_eq!(
            resize_from_corner(s, Corner::TopLeft, 50, 50, (75, 50), (0, 0)),
            Rect::new(150, 150, 150, 150)
        );
        assert_eq!(
            resize_from_corner(s, Corner::TopLeft, 190, 0, (75, 50), (0, 0)),
            Rect::new(225, 100, 75, 200)
        );
        assert_eq!(
            resize_from_corner(s, Corner::BottomRight, 500, 0, (75, 50), (300, 300)).w,
            300
        );
    }
}
