//! Pure diffing of the desired arrangement against what has already been applied.
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

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

/// Give up waiting for the shown windows to become visible after this long.
const DEFER_TIMEOUT: Duration = Duration::from_millis(150);
/// Once the shown windows are visible, wait this long so DWM composes a frame with them.
const DEFER_SETTLE: Duration = Duration::from_millis(16);

/// Hides held back until the windows shown in the same flush are actually visible, so
/// the desktop never shows through between the outgoing and the incoming window.
#[derive(Default)]
pub struct DeferredHides {
    hides: Vec<WindowId>,
    awaiting: Vec<WindowId>,
    since: Option<Instant>,
    /// When every awaited window was first seen visible (reset if that stops holding).
    visible_since: Option<Instant>,
}

impl DeferredHides {
    pub fn is_empty(&self) -> bool {
        self.hides.is_empty()
    }

    /// Holds `hides` back until `awaiting` are visible; merges with pending hides and
    /// restarts the wait.
    pub fn defer(&mut self, hides: Vec<WindowId>, awaiting: Vec<WindowId>, now: Instant) {
        for id in hides {
            if !self.hides.contains(&id) {
                self.hides.push(id);
            }
        }
        self.awaiting = awaiting;
        self.since = Some(now);
        self.visible_since = None;
    }

    /// A window that is shown or placed again must stay visible: drop its pending hide.
    pub fn cancel(&mut self, id: WindowId) {
        self.hides.retain(|h| *h != id);
    }

    /// Whether the pending hides may be issued now.
    pub fn ready(&mut self, now: Instant, visible: impl Fn(WindowId) -> bool) -> bool {
        if self.hides.is_empty() {
            return false;
        }
        let since = self.since.unwrap_or(now);
        if now.saturating_duration_since(since) >= DEFER_TIMEOUT {
            return true;
        }
        if !self.awaiting.iter().all(|id| visible(*id)) {
            self.visible_since = None;
            return false;
        }
        let first = *self.visible_since.get_or_insert(now);
        now.saturating_duration_since(first) >= DEFER_SETTLE
    }

    /// Takes the pending hides and resets.
    pub fn take(&mut self) -> Vec<WindowId> {
        self.awaiting.clear();
        self.since = None;
        self.visible_since = None;
        std::mem::take(&mut self.hides)
    }

    pub fn clear(&mut self) {
        self.take();
    }
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

    #[test]
    fn deferred_hides_wait_for_visibility_and_settle() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut d = DeferredHides::default();
        assert!(!d.ready(t0, |_| true));
        d.defer(vec![WindowId(1)], vec![WindowId(2)], t0);
        assert!(!d.is_empty());
        assert!(!d.ready(t0 + ms(8), |_| false));
        // visible from 16 ms on, ready only 16 ms later
        assert!(!d.ready(t0 + ms(16), |_| true));
        assert!(!d.ready(t0 + ms(24), |_| true));
        assert!(d.ready(t0 + ms(32), |_| true));
        assert_eq!(d.take(), vec![WindowId(1)]);
        assert!(d.is_empty());
    }

    #[test]
    fn deferred_hides_visibility_loss_resets_settle() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut d = DeferredHides::default();
        d.defer(vec![WindowId(1)], vec![WindowId(2)], t0);
        assert!(!d.ready(t0 + ms(8), |_| true));
        assert!(!d.ready(t0 + ms(16), |_| false));
        assert!(!d.ready(t0 + ms(24), |_| true));
        assert!(!d.ready(t0 + ms(32), |_| true));
        assert!(d.ready(t0 + ms(40), |_| true));
    }

    #[test]
    fn deferred_hides_time_out() {
        let t0 = Instant::now();
        let mut d = DeferredHides::default();
        d.defer(vec![WindowId(1)], vec![WindowId(2)], t0);
        assert!(!d.ready(t0 + Duration::from_millis(149), |_| false));
        assert!(d.ready(t0 + Duration::from_millis(150), |_| false));
    }

    #[test]
    fn deferred_hide_cancelled_by_reshow() {
        let t0 = Instant::now();
        let mut d = DeferredHides::default();
        d.defer(vec![WindowId(1), WindowId(3)], vec![WindowId(2)], t0);
        d.cancel(WindowId(1));
        assert_eq!(d.take(), vec![WindowId(3)]);
        d.defer(vec![WindowId(1)], vec![WindowId(2)], t0);
        d.cancel(WindowId(1));
        assert!(d.is_empty());
        assert!(!d.ready(t0 + Duration::from_secs(1), |_| true));
    }

    #[test]
    fn deferred_hides_merge_and_restart() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut d = DeferredHides::default();
        d.defer(vec![WindowId(1)], vec![WindowId(2)], t0);
        d.defer(
            vec![WindowId(1), WindowId(3)],
            vec![WindowId(4)],
            t0 + ms(100),
        );
        // restarted: the old timeout no longer applies
        assert!(!d.ready(t0 + ms(150), |id| id == WindowId(2)));
        assert!(d.ready(t0 + ms(250), |_| false));
        assert_eq!(d.take(), vec![WindowId(1), WindowId(3)]);
    }
}
