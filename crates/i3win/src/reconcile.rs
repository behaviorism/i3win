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
}

/// Diffs `arr` against what was already applied and returns the minimal ops.
/// Order: hides first (avoids flashing), then shows, then placements.
pub fn plan(applied: &mut Applied, arr: &Arrangement, managed: &HashSet<WindowId>) -> Vec<WinOp> {
    let mut hides = Vec::new();
    let mut shows = Vec::new();
    let mut places = Vec::new();
    for p in &arr.windows {
        if applied.hidden.remove(&p.id) {
            shows.push(WinOp::Show(p.id));
        }
        if applied.rects.get(&p.id) != Some(&p.rect) {
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
    hides.extend(shows);
    hides.extend(places);
    hides
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
                WinOp::Hide(WindowId(2)),
                WinOp::Place {
                    id: WindowId(1),
                    rect: Rect::new(0, 0, 10, 10),
                    raise: false
                }
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
    fn unhide_shows_then_places() {
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
                WinOp::Show(WindowId(1)),
                WinOp::Place {
                    id: WindowId(1),
                    rect: Rect::new(0, 0, 5, 5),
                    raise: true
                }
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
    fn forgets_unmanaged() {
        let mut a = Applied::default();
        plan(&mut a, &arr(vec![], vec![1]), &managed(&[1]));
        plan(&mut a, &arr(vec![], vec![]), &managed(&[]));
        assert!(a.hidden.is_empty());
    }
}
