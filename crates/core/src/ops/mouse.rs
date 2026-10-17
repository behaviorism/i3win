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

impl Wm {
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
