//! Pure mouse drag state machine: floating move/resize, tiled edge resize and
//! tiling drag (drop preview). Drives the core geometry functions; the App feeds it
//! hook events and applies the result with a flush.
use i3win_core::geom::{Direction, Rect};
use i3win_core::tree::{NodeId, NodeKind};
use i3win_core::wm::Wm;
use i3win_core::{Corner, DropTarget};

use crate::win::mouse::Button;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragKind {
    MoveFloating { start: Rect },
    ResizeFloating { start: Rect, corner: Corner },
    ResizeTiled { h: Direction, v: Direction },
    Tiling,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drag {
    pub node: NodeId,
    pub kind: DragKind,
    pub origin: (i32, i32),
    pub last: (i32, i32),
    pub last_apply_ms: u64,
}

/// Tiled live resize applies at most once per this many milliseconds.
const TILED_RESIZE_INTERVAL_MS: u64 = 16;

fn floating_rect(wm: &Wm, node: NodeId) -> Option<Rect> {
    match &wm.tree.get(node).kind {
        NodeKind::Window(w) => w.floating_rect,
        _ => None,
    }
}

/// The drag a press of `button` at (x, y) on window `node` starts, if any.
/// Floating: Left moves, Right resizes from the nearest corner. Tiled: Left is a
/// tiling drag when `tiling_drag` allows it for this kind of press (modifier or
/// title bar), Right resizes the nearest edges.
pub fn start_drag(
    wm: &Wm,
    node: NodeId,
    button: Button,
    x: i32,
    y: i32,
    from_titlebar: bool,
) -> Option<Drag> {
    if !wm.tree.contains(node) || !matches!(wm.tree.get(node).kind, NodeKind::Window(_)) {
        return None;
    }
    let kind = if wm.tree.is_floating(node) {
        let start = floating_rect(wm, node)?;
        match button {
            Button::Left => DragKind::MoveFloating { start },
            Button::Right => DragKind::ResizeFloating {
                start,
                corner: i3win_core::corner_for_press(start, x, y),
            },
            Button::Middle => return None,
        }
    } else {
        match button {
            Button::Left => {
                let td = wm.config.tiling_drag;
                let allowed = if from_titlebar {
                    td.titlebar
                } else {
                    td.modifier
                };
                if !allowed {
                    return None;
                }
                DragKind::Tiling
            }
            Button::Right => {
                let (h, v) = i3win_core::edges_for_press(wm.node_rect(node)?, x, y);
                DragKind::ResizeTiled { h, v }
            }
            Button::Middle => return None,
        }
    };
    Some(Drag {
        node,
        kind,
        origin: (x, y),
        last: (x, y),
        last_apply_ms: 0,
    })
}

/// Edge delta for a pointer motion: positive moves the edge outward.
fn outward(dir: Direction, dx: i32, dy: i32) -> i32 {
    match dir {
        Direction::Right => dx,
        Direction::Left => -dx,
        Direction::Down => dy,
        Direction::Up => -dy,
    }
}

/// Applies the pointer at (x, y); returns whether the arrangement changed.
pub fn drag_step(wm: &mut Wm, d: &mut Drag, x: i32, y: i32, now_ms: u64) -> bool {
    if !wm.tree.contains(d.node) {
        return false;
    }
    let (dx, dy) = (x.saturating_sub(d.origin.0), y.saturating_sub(d.origin.1));
    match d.kind {
        DragKind::MoveFloating { start } => {
            let Some(id) = wm.tree.window_id(d.node) else {
                return false;
            };
            let r = Rect::new(
                start.x.saturating_add(dx),
                start.y.saturating_add(dy),
                start.w,
                start.h,
            );
            let before = floating_rect(wm, d.node);
            wm.on_floating_moved(id, r);
            d.last = (x, y);
            floating_rect(wm, d.node) != before
        }
        DragKind::ResizeFloating { start, corner } => {
            let Some(id) = wm.tree.window_id(d.node) else {
                return false;
            };
            let r = i3win_core::resize_from_corner(
                start,
                corner,
                dx,
                dy,
                wm.config.floating_minimum_size,
                wm.config.floating_maximum_size,
            );
            let before = floating_rect(wm, d.node);
            wm.on_floating_moved(id, r);
            d.last = (x, y);
            floating_rect(wm, d.node) != before
        }
        DragKind::ResizeTiled { h, v } => {
            if now_ms.saturating_sub(d.last_apply_ms) < TILED_RESIZE_INTERVAL_MS {
                return false;
            }
            let (mx, my) = (x.saturating_sub(d.last.0), y.saturating_sub(d.last.1));
            if (mx, my) == (0, 0) {
                return false;
            }
            let a = mx != 0 && wm.resize_edge(d.node, h, outward(h, mx, my));
            let b = my != 0 && wm.resize_edge(d.node, v, outward(v, mx, my));
            d.last = (x, y);
            d.last_apply_ms = now_ms;
            a || b
        }
        DragKind::Tiling => {
            d.last = (x, y);
            false
        }
    }
}

/// Whether a throttled tiled resize still owes the move to (x, y) (the pointer
/// stopped within the throttle interval): the caller re-applies it later.
pub fn pending_step(d: &Drag, x: i32, y: i32) -> bool {
    matches!(d.kind, DragKind::ResizeTiled { .. }) && d.last != (x, y)
}

/// Detects a missed button-up from the async button state. A press the mouse hook
/// swallowed never reaches the async state, so its button reads "up" for the whole
/// drag: the watch arms only once the button has been seen down during this drag,
/// and hook-swallowed drags end only on the hook's Release (or Escape).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReleaseWatch {
    armed: bool,
}

impl ReleaseWatch {
    /// Feeds the async state of the drag's button at a move; true = release missed.
    pub fn on_move(&mut self, button_down: bool) -> bool {
        if button_down {
            self.armed = true;
            false
        } else {
            self.armed
        }
    }
}

/// Ends a drag at its last position: a tiling drag drops onto `target`.
pub fn finish_drag(wm: &mut Wm, d: &Drag, target: Option<DropTarget>) {
    if let (DragKind::Tiling, Some(t)) = (d.kind, target) {
        wm.drop_window(d.node, &t);
    }
}

/// Per ancestor of a dragged node: its children and their percents.
pub type PercentSnapshot = Vec<(NodeId, Vec<(NodeId, f64)>)>;

/// The percents a tiled resize of `node` may change: every child of each ancestor.
pub fn snapshot_percents(wm: &Wm, node: NodeId) -> PercentSnapshot {
    let mut out = Vec::new();
    let mut c = node;
    while let Some(p) = wm.tree.parent(c) {
        let pn = wm.tree.get(p);
        if !matches!(pn.kind, NodeKind::Workspace { .. } | NodeKind::Container) {
            break;
        }
        let kids = pn
            .children
            .iter()
            .map(|&s| (s, wm.tree.get(s).percent))
            .collect();
        out.push((p, kids));
        c = p;
    }
    out
}

/// Undoes a drag (Escape): floating rect back to its start, tiled percents back to
/// the snapshot taken at the start (only where the children are still the same).
/// A tiling drag changed nothing.
pub fn cancel_drag(wm: &mut Wm, d: &Drag, percents: &PercentSnapshot) {
    match d.kind {
        DragKind::MoveFloating { start } | DragKind::ResizeFloating { start, .. } => {
            if let Some(id) = wm.tree.window_id(d.node) {
                wm.on_floating_moved(id, start);
            }
        }
        DragKind::ResizeTiled { .. } => {
            for (p, kids) in percents {
                let same = wm.tree.contains(*p)
                    && wm
                        .tree
                        .get(*p)
                        .children
                        .iter()
                        .copied()
                        .eq(kids.iter().map(|k| k.0));
                if same {
                    for &(n, pct) in kids {
                        wm.tree.get_mut(n).percent = pct;
                    }
                }
            }
        }
        DragKind::Tiling => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use i3win_core::config::{parse_config, Config};
    use i3win_core::tree::{WindowId, WindowInfo};
    use i3win_core::wm::OutputInfo;
    use i3win_core::DropZone;

    fn out(name: &str, x: i32, w: i32) -> OutputInfo {
        OutputInfo {
            name: name.into(),
            rect: Rect::new(x, 0, w, 500),
            scale: 1.0,
            primary: false,
        }
    }
    fn win(n: isize) -> WindowInfo {
        WindowInfo {
            id: WindowId(n),
            title: format!("w{n}"),
            class: format!("c{n}"),
            instance: format!("i{n}"),
            rect: Rect::new(0, 0, 400, 300),
            wants_float: false,
        }
    }
    fn test_wm() -> Wm {
        let mut config = Config::default();
        config.font.size_pt = 9.0;
        Wm::new(config, vec![out("O1", 0, 1000)])
    }
    fn ws_desc(wm: &Wm) -> String {
        wm.tree.describe(wm.focused_workspace())
    }

    #[test]
    fn floating_move_and_resize() {
        let mut wm = test_wm();
        wm.manage(win(1));
        wm.run_command("floating enable, move position 100 px 100 px");
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        let mut d = start_drag(&wm, n, Button::Left, 150, 150, false).unwrap();
        drag_step(&mut wm, &mut d, 170, 140, 0);
        let fr = |wm: &Wm| match &wm.tree.get(n).kind {
            NodeKind::Window(x) => x.floating_rect.unwrap(),
            _ => unreachable!(),
        };
        assert_eq!((fr(&wm).x, fr(&wm).y), (120, 90));
        let start = fr(&wm);
        let mut d = start_drag(
            &wm,
            n,
            Button::Right,
            start.right() - 1,
            start.bottom() - 1,
            false,
        )
        .unwrap();
        drag_step(&mut wm, &mut d, start.right() + 49, start.bottom() + 9, 0);
        assert_eq!((fr(&wm).w, fr(&wm).h), (start.w + 50, start.h + 10));
    }
    #[test]
    fn tiled_resize_throttled_and_tiling_drop() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        wm.manage(win(1));
        wm.manage(win(2));
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let mut d = start_drag(&wm, n1, Button::Right, 400, 100, false).unwrap(); // right edge, top edge
        assert!(drag_step(&mut wm, &mut d, 500, 100, 100));
        assert!((wm.tree.get(n1).percent - 0.6).abs() < 1e-9);
        assert!(!drag_step(&mut wm, &mut d, 600, 100, 105)); // < 16 ms later: skipped
        assert!(pending_step(&d, 600, 100));
        assert!(drag_step(&mut wm, &mut d, 600, 100, 116)); // the timer applies it
        assert!(!pending_step(&d, 600, 100));
        assert!((wm.tree.get(n1).percent - 0.7).abs() < 1e-9);
        let t = start_drag(&wm, n1, Button::Left, 10, 10, false).unwrap();
        let n2 = wm.tree.window_node(WindowId(2)).unwrap();
        finish_drag(
            &mut wm,
            &t,
            Some(DropTarget::Window(n2, DropZone::Side(Direction::Right))),
        );
        assert_eq!(ws_desc(&wm), "H[w2 w1]");
    }
    #[test]
    fn cancel_restores_floating_and_tiled() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        wm.manage(win(1));
        wm.manage(win(2));
        let n1 = wm.tree.window_node(WindowId(1)).unwrap();
        let snap = snapshot_percents(&wm, n1);
        let mut d = start_drag(&wm, n1, Button::Right, 400, 100, false).unwrap();
        assert!(drag_step(&mut wm, &mut d, 500, 100, 100));
        cancel_drag(&mut wm, &d, &snap);
        assert!((wm.tree.get(n1).percent - 0.5).abs() < 1e-9);

        wm.manage(win(3));
        wm.run_command("floating enable, move position 100 px 100 px");
        let n3 = wm.tree.window_node(WindowId(3)).unwrap();
        let start = floating_rect(&wm, n3).unwrap();
        let mut d = start_drag(&wm, n3, Button::Left, 150, 150, false).unwrap();
        assert!(drag_step(&mut wm, &mut d, 250, 260, 0));
        assert_ne!(floating_rect(&wm, n3), Some(start));
        let snap3 = snapshot_percents(&wm, n3);
        cancel_drag(&mut wm, &d, &snap3);
        assert_eq!(floating_rect(&wm, n3), Some(start));
    }
    #[test]
    fn right_and_middle_on_tiled_and_titlebar_float() {
        let mut wm = test_wm();
        wm.config.bars.clear();
        wm.manage(win(1));
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        assert!(start_drag(&wm, n, Button::Middle, 10, 10, false).is_none());
        // A lone window has no edge to move.
        let mut d = start_drag(&wm, n, Button::Right, 10, 10, false).unwrap();
        assert!(!drag_step(&mut wm, &mut d, 50, 50, 100));
        wm.run_command("floating enable");
        let d = start_drag(&wm, n, Button::Left, 10, 10, true).unwrap();
        assert!(matches!(d.kind, DragKind::MoveFloating { .. }));
    }
    #[test]
    fn release_watch_arms_only_after_seen_down() {
        // Hook-swallowed press: the async state never shows the button down.
        let mut w = ReleaseWatch::default();
        assert!(!w.on_move(false));
        assert!(!w.on_move(false));
        // Title-bar press: down, then up without a release message.
        let mut w = ReleaseWatch::default();
        assert!(!w.on_move(true));
        assert!(!w.on_move(true));
        assert!(w.on_move(false));
    }
    #[test]
    fn tiling_drag_respects_config() {
        let mut wm = Wm::new(
            parse_config("tiling_drag titlebar\n"),
            vec![out("O1", 0, 1000)],
        );
        wm.manage(win(1));
        let n = wm.tree.window_node(WindowId(1)).unwrap();
        assert!(start_drag(&wm, n, Button::Left, 10, 10, false).is_none());
        assert!(start_drag(&wm, n, Button::Left, 10, 10, true).is_some());
    }
}
