//! Pure diffing of the desired arrangement against what has already been applied.
use std::collections::{HashMap, HashSet};

use i3win_core::arrange::Arrangement;
use i3win_core::geom::Rect;
use i3win_core::tree::WindowId;

#[derive(Debug, PartialEq, Eq)]
pub enum WinOp {
    Show(WindowId),
    Hide(WindowId),
    Place {
        id: WindowId,
        rect: Rect,
        raise: bool,
    },
}

#[derive(Default)]
pub struct Applied {
    pub rects: HashMap<WindowId, Rect>,
    pub hidden: HashSet<WindowId>,
    /// Windows in a native move/size loop: never placed until they are thawed.
    pub frozen: HashSet<WindowId>,
}

/// Diffs `arr` against what was already applied and returns the minimal ops.
/// Order: placements first (hidden windows move while invisible), then shows, then
/// hides last so the screen is never empty mid-switch.
pub fn plan(applied: &mut Applied, arr: &Arrangement, managed: &HashSet<WindowId>) -> Vec<WinOp> {
    let mut hides = Vec::new();
    let mut shows = Vec::new();
    let mut places = Vec::new();
    for p in &arr.windows {
        if applied.hidden.remove(&p.id) {
            shows.push(WinOp::Show(p.id));
        }
        if !applied.frozen.contains(&p.id) && applied.rects.get(&p.id) != Some(&p.rect) {
            applied.rects.insert(p.id, p.rect);
            places.push(WinOp::Place {
                id: p.id,
                rect: p.rect,
                raise: p.floating,
            });
        }
    }
    for &id in &arr.hidden {
        if applied.hidden.insert(id) {
            applied.rects.remove(&id);
            hides.push(WinOp::Hide(id));
        }
    }
    applied.rects.retain(|id, _| managed.contains(id));
    applied.hidden.retain(|id| managed.contains(id));
    applied.frozen.retain(|id| managed.contains(id));
    places.extend(shows);
    places.extend(hides);
    places
}

/// Forgets the applied rect so the next plan re-places the window.
pub fn invalidate(applied: &mut Applied, id: WindowId) {
    applied.rects.remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use i3win_core::arrange::WindowPlacement;

    fn arr(windows: Vec<(isize, Rect, bool)>, hidden: Vec<isize>) -> Arrangement {
        Arrangement {
            windows: windows
                .into_iter()
                .map(|(i, r, f)| WindowPlacement {
                    id: WindowId(i),
                    rect: r,
                    floating: f,
                    fullscreen: false,
                })
                .collect(),
            hidden: hidden.into_iter().map(WindowId).collect(),
            decos: vec![],
            fullscreen_outputs: vec![],
            rects: vec![],
        }
    }
    fn managed(ids: &[isize]) -> HashSet<WindowId> {
        ids.iter().map(|&i| WindowId(i)).collect()
    }
    #[test]
    fn first_plan_places_everything_and_hides() {
        let mut a = Applied::default();
        let ops = plan(
            &mut a,
            &arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![2]),
            &managed(&[1, 2]),
        );
        assert_eq!(
            ops,
            vec![
                WinOp::Place {
                    id: WindowId(1),
                    rect: Rect::new(0, 0, 10, 10),
                    raise: false
                },
                WinOp::Hide(WindowId(2)),
            ]
        );
    }
    #[test]
    fn unchanged_plan_is_empty() {
        let mut a = Applied::default();
        let x = arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![2]);
        plan(&mut a, &x, &managed(&[1, 2]));
        assert!(plan(&mut a, &x, &managed(&[1, 2])).is_empty());
    }
    #[test]
    fn unhide_places_then_shows() {
        let mut a = Applied::default();
        plan(&mut a, &arr(vec![], vec![1]), &managed(&[1]));
        let ops = plan(
            &mut a,
            &arr(vec![(1, Rect::new(0, 0, 5, 5), true)], vec![]),
            &managed(&[1]),
        );
        assert_eq!(
            ops,
            vec![
                WinOp::Place {
                    id: WindowId(1),
                    rect: Rect::new(0, 0, 5, 5),
                    raise: true
                },
                WinOp::Show(WindowId(1)),
            ]
        );
    }
    #[test]
    fn workspace_switch_places_shows_then_hides() {
        let mut a = Applied::default();
        let old = arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![2]);
        plan(&mut a, &old, &managed(&[1, 2]));
        let new = arr(vec![(2, Rect::new(0, 0, 20, 20), false)], vec![1]);
        let ops = plan(&mut a, &new, &managed(&[1, 2]));
        assert_eq!(
            ops,
            vec![
                WinOp::Place {
                    id: WindowId(2),
                    rect: Rect::new(0, 0, 20, 20),
                    raise: false
                },
                WinOp::Show(WindowId(2)),
                WinOp::Hide(WindowId(1)),
            ]
        );
    }
    #[test]
    fn invalidate_forces_replace() {
        let mut a = Applied::default();
        let x = arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![]);
        plan(&mut a, &x, &managed(&[1]));
        invalidate(&mut a, WindowId(1));
        assert_eq!(plan(&mut a, &x, &managed(&[1])).len(), 1);
    }
    #[test]
    fn frozen_window_is_not_placed() {
        let mut a = Applied::default();
        a.frozen.insert(WindowId(1));
        let x = arr(vec![(1, Rect::new(0, 0, 10, 10), false)], vec![]);
        assert!(plan(&mut a, &x, &managed(&[1])).is_empty());
        a.frozen.clear();
        assert_eq!(plan(&mut a, &x, &managed(&[1])).len(), 1);
    }
    #[test]
    fn forgets_unmanaged() {
        let mut a = Applied::default();
        plan(&mut a, &arr(vec![], vec![1]), &managed(&[1]));
        plan(&mut a, &arr(vec![], vec![]), &managed(&[]));
        assert!(a.hidden.is_empty());
    }
}
