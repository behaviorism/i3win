use crate::command::{GapsOp, GapsScope, GapsWhich};
use crate::config::{Gaps, GapsOverride};
use crate::wm::Wm;

#[derive(Clone, Copy)]
enum Side {
    Inner,
    Top,
    Right,
    Bottom,
    Left,
}

fn sides(which: GapsWhich) -> &'static [Side] {
    match which {
        GapsWhich::Inner => &[Side::Inner],
        GapsWhich::Outer => &[Side::Top, Side::Right, Side::Bottom, Side::Left],
        GapsWhich::Horizontal => &[Side::Left, Side::Right],
        GapsWhich::Vertical => &[Side::Top, Side::Bottom],
        GapsWhich::Top => &[Side::Top],
        GapsWhich::Right => &[Side::Right],
        GapsWhich::Bottom => &[Side::Bottom],
        GapsWhich::Left => &[Side::Left],
    }
}

fn get(g: &Gaps, s: Side) -> i32 {
    match s {
        Side::Inner => g.inner,
        Side::Top => g.top,
        Side::Right => g.right,
        Side::Bottom => g.bottom,
        Side::Left => g.left,
    }
}

fn set(g: &mut Gaps, s: Side, v: i32) {
    match s {
        Side::Inner => g.inner = v,
        Side::Top => g.top = v,
        Side::Right => g.right = v,
        Side::Bottom => g.bottom = v,
        Side::Left => g.left = v,
    }
}

fn slot(o: &mut GapsOverride, s: Side) -> &mut Option<i32> {
    match s {
        Side::Inner => &mut o.inner,
        Side::Top => &mut o.top,
        Side::Right => &mut o.right,
        Side::Bottom => &mut o.bottom,
        Side::Left => &mut o.left,
    }
}

impl Wm {
    pub fn gaps_command(&mut self, which: GapsWhich, scope: GapsScope, op: GapsOp, px: i32) {
        let ws = self.focused_workspace();
        let effective = self.effective_gaps(ws);
        let affected = sides(which);
        // Multi-side toggle is decided by the first side of top/left.
        let toggle_off = match which {
            GapsWhich::Horizontal => get(&effective, Side::Left) != 0,
            GapsWhich::Outer | GapsWhich::Vertical => get(&effective, Side::Top) != 0,
            _ => get(&effective, affected[0]) != 0,
        };
        for &s in affected {
            let cur = get(&effective, s);
            let new = match op {
                GapsOp::Set => px,
                GapsOp::Plus => cur.saturating_add(px),
                GapsOp::Minus => cur.saturating_sub(px),
                GapsOp::Toggle => {
                    if toggle_off {
                        0
                    } else {
                        px
                    }
                }
            }
            .max(0);
            match scope {
                GapsScope::Current => *slot(&mut self.tree.get_mut(ws).gaps, s) = Some(new),
                GapsScope::All => {
                    set(&mut self.config.gaps, s, new);
                    let all = self.sorted_workspaces();
                    for w in all {
                        *slot(&mut self.tree.get_mut(w).gaps, s) = None;
                    }
                }
            }
        }
    }
}
