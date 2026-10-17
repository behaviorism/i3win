//! Pure decisions for focus-follows-mouse, mouse warping and move/size freezes.
use i3win_core::config::MouseWarping;
use i3win_core::geom::Rect;
use i3win_core::tree::{NodeId, WindowId};

/// How long the pointer must rest on a new window before it takes focus.
pub const HOVER_REST_MS: u64 = 50;
/// A move/size freeze with no END event is lifted after this long.
pub const FREEZE_TIMEOUT_MS: u64 = 10_000;

#[derive(Clone, Debug, Default)]
pub struct HoverState {
    /// The window the pointer is resting on and since when (ms).
    pub candidate: Option<(WindowId, u64)>,
    /// Our own warp put the pointer here: moves reporting exactly this point are ignored.
    pub ignore_until_move_from: Option<(i32, i32)>,
}

#[derive(Clone, Copy, Debug)]
pub struct HoverInput {
    /// Managed window (or its deco) under the pointer.
    pub under: Option<WindowId>,
    pub focused: Option<WindowId>,
    pub foreground_managed: bool,
    pub button_down: bool,
    pub dragging: bool,
    pub over_bar: bool,
    pub over_tab_row: bool,
    pub now_ms: u64,
    pub pos: (i32, i32),
}

/// Returns the window to focus now, if any. Never while a button is down, during a
/// drag, with an unmanaged foreground, over a bar or a tab row; a new `under`
/// (other than the focused one) becomes the candidate and fires after resting 50 ms.
pub fn hover_step(s: &mut HoverState, i: &HoverInput) -> Option<WindowId> {
    if i.button_down || i.dragging || !i.foreground_managed || i.over_bar || i.over_tab_row {
        s.candidate = None;
        return None;
    }
    if let Some(p) = s.ignore_until_move_from {
        if p == i.pos {
            return None;
        }
        s.ignore_until_move_from = None;
    }
    let under = match i.under {
        Some(u) if Some(u) != i.focused => u,
        _ => {
            s.candidate = None;
            return None;
        }
    };
    match s.candidate {
        Some((c, since)) if c == under => {
            if i.now_ms.saturating_sub(since) >= HOVER_REST_MS {
                s.candidate = None;
                Some(under)
            } else {
                None
            }
        }
        _ => {
            s.candidate = Some((under, i.now_ms));
            None
        }
    }
}

/// Where to put the pointer after a focus change: `Output` only when the output
/// changed, `Container` on every focus change, `None` never. The center of the
/// focused rect.
pub fn warp_target(
    prev_output: Option<NodeId>,
    new_output: Option<NodeId>,
    mode: MouseWarping,
    focus_changed: bool,
    focused_rect: Option<Rect>,
) -> Option<(i32, i32)> {
    let wanted = match mode {
        MouseWarping::None => false,
        MouseWarping::Output => prev_output != new_output,
        MouseWarping::Container => focus_changed,
    };
    if wanted {
        focused_rect.map(|r| r.center())
    } else {
        None
    }
}

/// Whether a window frozen by a native move/size loop must be thawed: no END
/// arrived and the foreground moved to another window, or the freeze is too old.
pub fn freeze_expired(
    frozen: WindowId,
    started_ms: u64,
    now_ms: u64,
    foreground: Option<WindowId>,
) -> bool {
    now_ms.saturating_sub(started_ms) >= FREEZE_TIMEOUT_MS
        || foreground.is_some_and(|f| f != frozen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(under: Option<isize>, now: u64) -> HoverInput {
        HoverInput {
            under: under.map(WindowId),
            focused: Some(WindowId(1)),
            foreground_managed: true,
            button_down: false,
            dragging: false,
            over_bar: false,
            over_tab_row: false,
            now_ms: now,
            pos: (10, 10),
        }
    }
    #[test]
    fn hover_rests_50ms() {
        let mut s = HoverState::default();
        assert_eq!(hover_step(&mut s, &inp(Some(2), 0)), None);
        assert_eq!(hover_step(&mut s, &inp(Some(2), 30)), None);
        assert_eq!(hover_step(&mut s, &inp(Some(2), 55)), Some(WindowId(2)));
        assert_eq!(hover_step(&mut s, &inp(Some(1), 60)), None); // already focused
    }
    #[test]
    fn hover_restarts_on_a_new_window() {
        let mut s = HoverState::default();
        hover_step(&mut s, &inp(Some(2), 0));
        assert_eq!(hover_step(&mut s, &inp(Some(3), 40)), None);
        assert_eq!(hover_step(&mut s, &inp(Some(3), 80)), None);
        assert_eq!(hover_step(&mut s, &inp(Some(3), 95)), Some(WindowId(3)));
    }
    #[test]
    fn hover_blocked_when_foreground_unmanaged() {
        let mut s = HoverState::default();
        let mut i = inp(Some(2), 0);
        i.foreground_managed = false;
        hover_step(&mut s, &i);
        i.now_ms = 100;
        assert_eq!(hover_step(&mut s, &i), None);
        let mut b = inp(Some(2), 200);
        b.button_down = true;
        assert_eq!(hover_step(&mut s, &b), None);
    }
    #[test]
    fn hover_blocked_over_bar_tabs_and_drags() {
        for f in [
            |i: &mut HoverInput| i.over_bar = true,
            |i: &mut HoverInput| i.over_tab_row = true,
            |i: &mut HoverInput| i.dragging = true,
        ] {
            let mut s = HoverState::default();
            hover_step(&mut s, &inp(Some(2), 0));
            let mut i = inp(Some(2), 100);
            f(&mut i);
            assert_eq!(hover_step(&mut s, &i), None);
            assert_eq!(s.candidate, None);
        }
    }
    #[test]
    fn hover_ignores_our_warp_position() {
        let mut s = HoverState {
            ignore_until_move_from: Some((10, 10)),
            ..Default::default()
        };
        assert_eq!(hover_step(&mut s, &inp(Some(2), 0)), None);
        assert_eq!(s.candidate, None);
        let mut i = inp(Some(2), 10);
        i.pos = (11, 10);
        hover_step(&mut s, &i);
        assert_eq!(s.ignore_until_move_from, None);
        assert!(s.candidate.is_some());
    }
    #[test]
    fn warp_rules() {
        let r = Some(Rect::new(0, 0, 100, 50));
        assert_eq!(
            warp_target(
                Some(NodeId(1)),
                Some(NodeId(2)),
                MouseWarping::Output,
                true,
                r
            ),
            Some((50, 25))
        );
        assert_eq!(
            warp_target(
                Some(NodeId(1)),
                Some(NodeId(1)),
                MouseWarping::Output,
                true,
                r
            ),
            None
        );
        assert_eq!(
            warp_target(
                Some(NodeId(1)),
                Some(NodeId(1)),
                MouseWarping::Container,
                true,
                r
            ),
            Some((50, 25))
        );
        assert_eq!(
            warp_target(
                Some(NodeId(1)),
                Some(NodeId(1)),
                MouseWarping::Container,
                false,
                r
            ),
            None
        );
        assert_eq!(
            warp_target(
                Some(NodeId(1)),
                Some(NodeId(2)),
                MouseWarping::None,
                true,
                r
            ),
            None
        );
    }
    #[test]
    fn freeze_expiry() {
        let w = WindowId(1);
        assert!(!freeze_expired(w, 0, 5_000, Some(w)));
        assert!(!freeze_expired(w, 0, 5_000, None));
        assert!(freeze_expired(w, 0, 5_000, Some(WindowId(2))));
        assert!(freeze_expired(w, 0, 10_000, Some(w)));
        assert!(freeze_expired(w, 1_000, 11_000, None));
    }
}
