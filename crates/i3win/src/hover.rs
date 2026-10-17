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
    /// The foreground thread is in a menu, a popup (combo dropdown), a move/size
    /// loop or holds the capture: moving focus would close or break it.
    pub menu_active: bool,
    pub now_ms: u64,
    pub pos: (i32, i32),
}

/// Returns the window to focus now, if any. Never while a button is down, during a
/// drag, with an unmanaged foreground or one in a menu, over a bar or a tab row; a new `under`
/// (other than the focused one) becomes the candidate and fires after resting 50 ms.
pub fn hover_step(s: &mut HoverState, i: &HoverInput) -> Option<WindowId> {
    if i.button_down
        || i.dragging
        || !i.foreground_managed
        || i.menu_active
        || i.over_bar
        || i.over_tab_row
    {
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

/// `GetGUIThreadInfo` flags of a thread in a menu, popup menu, system menu or
/// move/size loop (GUI_INMENUMODE | GUI_POPUPMENUMODE | GUI_SYSTEMMENUMODE | GUI_INMOVESIZE).
const MENU_FLAGS: u32 = 0x4 | 0x10 | 0x8 | 0x2;

/// Whether the foreground thread's GUI state forbids a hover focus change: a menu
/// mode flag, a capture window or a menu owner.
pub fn menu_active(flags: u32, has_capture: bool, has_menu_owner: bool) -> bool {
    flags & MENU_FLAGS != 0 || has_capture || has_menu_owner
}

/// Where to put the pointer after a focus change, never when it is already where
/// it belongs: `Output` when the pointer is on another output than the newly
/// focused one, `Container` when it is outside the focused rect, `None` never.
/// The center of the focused rect.
pub fn warp_target(
    mode: MouseWarping,
    focus_changed: bool,
    pointer: Option<(i32, i32)>,
    pointer_output: Option<NodeId>,
    new_output: Option<NodeId>,
    focused_rect: Option<Rect>,
) -> Option<(i32, i32)> {
    let rect = focused_rect?;
    let wanted = focus_changed
        && match mode {
            MouseWarping::None => false,
            MouseWarping::Output => pointer_output != new_output,
            MouseWarping::Container => !pointer.is_some_and(|(x, y)| {
                x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
            }),
        };
    wanted.then(|| rect.center())
}

/// What to do with a native move/size loop that has not reported its END.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FreezeVerdict {
    Keep,
    /// The freeze is too old: lift it, but keep the START rect so a late END still
    /// applies the user's move/resize.
    Thaw,
    /// The foreground moved to a window that is neither the frozen one nor the
    /// foreground at START: the loop is over, forget it.
    Drop,
}

/// Judges a loop started at `started_ms`; `thawed` = already thawed by timeout.
pub fn freeze_verdict(
    frozen: WindowId,
    started_ms: u64,
    now_ms: u64,
    start_foreground: Option<WindowId>,
    foreground: Option<WindowId>,
    thawed: bool,
) -> FreezeVerdict {
    if foreground.is_some_and(|f| f != frozen && Some(f) != start_foreground) {
        FreezeVerdict::Drop
    } else if !thawed && now_ms.saturating_sub(started_ms) >= FREEZE_TIMEOUT_MS {
        FreezeVerdict::Thaw
    } else {
        FreezeVerdict::Keep
    }
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
            menu_active: false,
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
            |i: &mut HoverInput| i.menu_active = true,
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
    fn menu_state_blocks_hover() {
        assert!(!menu_active(0, false, false));
        assert!(!menu_active(0x1, false, false)); // GUI_CARETBLINKING only
        for f in [0x2, 0x4, 0x8, 0x10] {
            assert!(menu_active(f, false, false));
        }
        assert!(menu_active(0, true, false));
        assert!(menu_active(0, false, true));
    }
    #[test]
    fn warp_rules() {
        use MouseWarping::{Container, Output};
        let r = Some(Rect::new(0, 0, 100, 50));
        let (a, b) = (Some(NodeId(1)), Some(NodeId(2)));
        let inside = Some((10, 10));
        let outside = Some((500, 10));
        // Output: only when the pointer is on another output than the new focus.
        assert_eq!(warp_target(Output, true, outside, a, b, r), Some((50, 25)));
        assert_eq!(warp_target(Output, true, outside, b, b, r), None);
        assert_eq!(warp_target(Output, false, outside, a, b, r), None);
        // Container: only when the pointer is outside the focused rect.
        assert_eq!(
            warp_target(Container, true, outside, a, a, r),
            Some((50, 25))
        );
        assert_eq!(warp_target(Container, true, inside, a, a, r), None);
        assert_eq!(warp_target(Container, true, None, a, a, r), Some((50, 25)));
        assert_eq!(warp_target(Container, false, outside, a, a, r), None);
        assert_eq!(
            warp_target(MouseWarping::None, true, outside, a, b, r),
            None
        );
        assert_eq!(warp_target(Output, true, outside, a, b, None), None);
    }
    #[test]
    fn freeze_verdicts() {
        use FreezeVerdict::*;
        let w = WindowId(1);
        let o = Some(WindowId(2));
        assert_eq!(freeze_verdict(w, 0, 5_000, None, Some(w), false), Keep);
        assert_eq!(freeze_verdict(w, 0, 5_000, None, None, false), Keep);
        assert_eq!(freeze_verdict(w, 0, 5_000, None, o, false), Drop);
        assert_eq!(freeze_verdict(w, 0, 10_000, None, Some(w), false), Thaw);
        assert_eq!(freeze_verdict(w, 1_000, 11_000, None, None, false), Thaw);
        // Thawed once: kept (for a late END) until the foreground moves away.
        assert_eq!(freeze_verdict(w, 0, 60_000, None, Some(w), true), Keep);
        assert_eq!(freeze_verdict(w, 0, 60_000, None, o, true), Drop);
        // The foreground at START (the dragged window not foreground yet) is not a change.
        assert_eq!(freeze_verdict(w, 0, 5_000, o, o, false), Keep);
        assert_eq!(
            freeze_verdict(w, 0, 5_000, o, Some(WindowId(3)), false),
            Drop
        );
    }
}
