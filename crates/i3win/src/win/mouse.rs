//! Low-level mouse hook plus a pure matcher that decides which buttons start a drag.
//!
//! The hook runs on the main thread and only reads the thread-local snapshot the main
//! thread writes each flush: no allocation in the common path, no cross-process calls.
use std::cell::{Cell, RefCell};

use i3win_core::geom::Rect;
use i3win_core::keys::Modifiers;
use i3win_core::tree::WindowId;
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, MSLLHOOKSTRUCT,
    WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
    WM_RBUTTONDOWN, WM_RBUTTONUP,
};

use crate::mainloop::{main_hwnd, WM_APP_MOUSE};
use crate::win::keyboard::{self, OWN_INPUT_TAG};

const HC_ACTION: i32 = 0;
const LLMHF_INJECTED: u32 = 0x01;
/// Button events that fit before the main thread drains, reserved up front so the
/// hook does not allocate.
const QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub id: WindowId,
    pub rect: Rect,
    pub floating: bool,
}

impl Hit {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.rect.x && x < self.rect.right() && y >= self.rect.y && y < self.rect.bottom()
    }
}

/// Managed window rects as of the last flush.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Topmost first: floating (last-focused first), then tiled.
    pub hits: Vec<Hit>,
    /// `floating_modifier`; `None` disables modifier drags.
    pub modifier: Option<Modifiers>,
    /// Post moves even without a drag (focus follows mouse is on).
    pub want_moves: bool,
    /// The foreground window is managed. When false (Start menu, a context menu, an
    /// unmanaged or elevated window in front) presses never start a drag, since an
    /// unmanaged window may cover a managed rect.
    pub foreground_managed: bool,
}

/// Whether a swallowed modifier-click must be followed by the menu-masking dummy key:
/// Alt or Win going down and up with no input between opens the menu bar / Start.
pub fn needs_menu_mask(mods: Modifiers) -> bool {
    mods.contains(Modifiers::ALT) || mods.contains(Modifiers::WIN)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    Press {
        button: Button,
        x: i32,
        y: i32,
        id: WindowId,
        /// Modifiers held at the press.
        mods: Modifiers,
    },
    Release {
        button: Button,
        x: i32,
        y: i32,
    },
    Move {
        x: i32,
        y: i32,
    },
}

#[derive(Debug)]
pub enum MouseAction {
    Pass,
    Swallow,
}

#[derive(Default)]
pub struct MouseMatcher {
    snapshot: Snapshot,
    /// The button of the drag this matcher started, until that button's release.
    dragging: Option<Button>,
}

impl MouseMatcher {
    /// Replaces the snapshot; a drag in progress keeps going.
    pub fn set_snapshot(&mut self, s: Snapshot) {
        self.snapshot = s;
    }

    pub fn on_button(
        &mut self,
        button: Button,
        down: bool,
        x: i32,
        y: i32,
        mods: Modifiers,
    ) -> (MouseAction, Option<MouseEvent>) {
        if !down {
            if self.dragging == Some(button) {
                self.dragging = None;
                return (
                    MouseAction::Swallow,
                    Some(MouseEvent::Release { button, x, y }),
                );
            }
            return (MouseAction::Pass, None);
        }
        // One drag at a time; Middle never starts one; nothing over an unmanaged foreground.
        if self.dragging.is_some() || button == Button::Middle || !self.snapshot.foreground_managed
        {
            return (MouseAction::Pass, None);
        }
        let held = self
            .snapshot
            .modifier
            .is_some_and(|m| m.0 != 0 && mods.contains(m));
        if !held {
            return (MouseAction::Pass, None);
        }
        match self.snapshot.hits.iter().find(|h| h.contains(x, y)) {
            Some(hit) => {
                self.dragging = Some(button);
                (
                    MouseAction::Swallow,
                    Some(MouseEvent::Press {
                        button,
                        x,
                        y,
                        id: hit.id,
                        mods,
                    }),
                )
            }
            None => (MouseAction::Pass, None),
        }
    }

    /// `Some(Move)` iff a drag is in progress or the snapshot wants moves.
    pub fn on_move(&mut self, x: i32, y: i32) -> Option<MouseEvent> {
        (self.dragging.is_some() || self.snapshot.want_moves).then_some(MouseEvent::Move { x, y })
    }

    #[cfg_attr(not(test), allow(dead_code))] // read by the drag state machine (Task 4)
    pub fn is_dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// Updates only the foreground flag of the snapshot (Foreground events).
    pub fn set_foreground_managed(&mut self, managed: bool) {
        self.snapshot.foreground_managed = managed;
    }

    /// Ends a drag whose button-up was missed (the App saw the button no longer down):
    /// clears `dragging` and returns the Release at (x, y); `None` without a drag.
    #[cfg_attr(not(test), allow(dead_code))] // used by the drag state machine (Task 4)
    pub fn force_release(&mut self, x: i32, y: i32) -> Option<MouseEvent> {
        let button = self.dragging.take()?;
        Some(MouseEvent::Release { button, x, y })
    }
}

thread_local! {
    static MATCHER: RefCell<MouseMatcher> = RefCell::new(MouseMatcher::default());
    /// Button events in order; a coalesced move that preceded one is queued before it.
    static QUEUE: RefCell<Vec<MouseEvent>> = RefCell::new(Vec::with_capacity(QUEUE_CAPACITY));
    /// Latest wanted move since the last queued button event (latest wins).
    static LATEST_MOVE: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
    /// True while a WM_APP_MOUSE is posted and not yet drained.
    static MOUSE_PENDING: Cell<bool> = const { Cell::new(false) };
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
}

/// Posts one WM_APP_MOUSE unless one is already pending.
fn wake() {
    if MOUSE_PENDING.with(|p| p.get()) {
        return;
    }
    let main = main_hwnd();
    if main == 0 {
        return;
    }
    // SAFETY: plain Win32 call; posting never blocks the hook.
    let posted =
        unsafe { PostMessageW(HWND(main as *mut _), WM_APP_MOUSE, WPARAM(0), LPARAM(0)).is_ok() };
    if posted {
        MOUSE_PENDING.with(|p| p.set(true));
    }
}

/// Queues a button event after the move it followed; false if the queue is busy.
fn queue(ev: MouseEvent) -> bool {
    QUEUE.with(|q| match q.try_borrow_mut() {
        Ok(mut q) => {
            if let Some((x, y)) = LATEST_MOVE.with(|m| m.take()) {
                q.push(MouseEvent::Move { x, y });
            }
            q.push(ev);
            true
        }
        Err(_) => false,
    })
}

fn button_of(msg: u32) -> Option<(Button, bool)> {
    match msg {
        WM_LBUTTONDOWN => Some((Button::Left, true)),
        WM_LBUTTONUP => Some((Button::Left, false)),
        WM_RBUTTONDOWN => Some((Button::Right, true)),
        WM_RBUTTONUP => Some((Button::Right, false)),
        WM_MBUTTONDOWN => Some((Button::Middle, true)),
        WM_MBUTTONUP => Some((Button::Middle, false)),
        _ => None,
    }
}

unsafe extern "system" fn ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION && lparam.0 != 0 {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let own = info.flags & LLMHF_INJECTED != 0 && info.dwExtraInfo == OWN_INPUT_TAG;
        let (x, y) = (info.pt.x, info.pt.y);
        let msg = wparam.0 as u32;
        if !own && msg == WM_MOUSEMOVE {
            let wanted = MATCHER.with(|m| {
                m.try_borrow_mut()
                    .ok()
                    .and_then(|mut m| m.on_move(x, y))
                    .is_some()
            });
            if wanted {
                LATEST_MOVE.with(|m| m.set(Some((x, y))));
                wake();
            }
        } else if let Some((button, down)) = button_of(msg).filter(|_| !own) {
            // Modifiers only matter for a press.
            let mods = if down {
                keyboard::held_mods()
            } else {
                Modifiers::default()
            };
            let (action, ev) = MATCHER.with(|m| match m.try_borrow_mut() {
                Ok(mut m) => m.on_button(button, down, x, y, mods),
                Err(_) => (MouseAction::Pass, None),
            });
            if let Some(ev) = ev {
                if queue(ev) {
                    wake();
                }
            }
            if let MouseAction::Swallow = action {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

pub fn install() -> Result<()> {
    if HOOK.with(|h| h.get().is_some()) {
        return Ok(());
    }
    let hmod = unsafe { GetModuleHandleW(None)? };
    let h = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(ll_proc), hmod, 0)? };
    HOOK.with(|c| c.set(Some(h)));
    Ok(())
}

pub fn uninstall() {
    if let Some(h) = HOOK.with(|c| c.take()) {
        unsafe {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

/// Main thread, each flush: the managed rects presses are tested against.
pub fn set_snapshot(s: Snapshot) {
    MATCHER.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.set_snapshot(s);
        }
    });
}

/// Main thread, on Foreground events between flushes.
pub fn set_foreground_managed(managed: bool) {
    MATCHER.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.set_foreground_managed(managed);
        }
    });
}

/// Button events in order (each preceded by the move it followed), then the latest
/// move after them. Clears the pending flag so the next hook event posts a new wake-up.
pub fn drain() -> (Vec<MouseEvent>, Option<(i32, i32)>) {
    MOUSE_PENDING.with(|p| p.set(false));
    let events = QUEUE.with(|q| {
        q.try_borrow_mut()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    });
    (events, LATEST_MOVE.with(|m| m.take()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot {
            hits: vec![
                Hit {
                    id: WindowId(2),
                    rect: Rect::new(100, 100, 100, 100),
                    floating: true,
                },
                Hit {
                    id: WindowId(1),
                    rect: Rect::new(0, 0, 500, 500),
                    floating: false,
                },
            ],
            modifier: Some(Modifiers::ALT),
            want_moves: false,
            foreground_managed: true,
        }
    }
    #[test]
    fn modifier_press_on_managed_is_swallowed() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        let (a, ev) = m.on_button(Button::Left, true, 150, 150, Modifiers::ALT);
        assert!(matches!(a, MouseAction::Swallow));
        assert_eq!(
            ev,
            Some(MouseEvent::Press {
                button: Button::Left,
                x: 150,
                y: 150,
                id: WindowId(2),
                mods: Modifiers::ALT
            })
        );
        assert_eq!(
            m.on_move(160, 160),
            Some(MouseEvent::Move { x: 160, y: 160 })
        );
        let (a, _) = m.on_button(Button::Left, false, 160, 160, Modifiers::default());
        assert!(matches!(a, MouseAction::Swallow));
        assert_eq!(m.on_move(1, 1), None);
    }
    #[test]
    fn press_outside_managed_passes() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        assert!(matches!(
            m.on_button(Button::Left, true, 900, 900, Modifiers::ALT).0,
            MouseAction::Pass
        ));
        assert!(matches!(
            m.on_button(Button::Left, true, 150, 150, Modifiers::default())
                .0,
            MouseAction::Pass
        ));
        assert!(matches!(
            m.on_button(Button::Middle, true, 150, 150, Modifiers::ALT)
                .0,
            MouseAction::Pass
        ));
        let mut none = snap();
        none.modifier = None;
        m.set_snapshot(none);
        assert!(matches!(
            m.on_button(Button::Left, true, 150, 150, Modifiers::ALT).0,
            MouseAction::Pass
        ));
    }
    #[test]
    fn modifier_release_does_not_end_drag() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        m.on_button(Button::Right, true, 50, 50, Modifiers::ALT);
        assert!(m.is_dragging());
        assert!(m.on_move(60, 60).is_some()); // Alt already released: still dragging
        assert!(matches!(
            m.on_button(Button::Right, false, 60, 60, Modifiers::default())
                .0,
            MouseAction::Swallow
        ));
        assert!(!m.is_dragging());
    }
    #[test]
    fn second_button_during_drag_passes_and_keeps_drag() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        m.on_button(Button::Left, true, 50, 50, Modifiers::ALT);
        assert!(matches!(
            m.on_button(Button::Right, true, 50, 50, Modifiers::ALT),
            (MouseAction::Pass, None)
        ));
        assert!(matches!(
            m.on_button(Button::Right, false, 50, 50, Modifiers::ALT),
            (MouseAction::Pass, None)
        ));
        assert!(m.is_dragging());
        assert_eq!(
            m.on_button(Button::Left, false, 70, 80, Modifiers::default())
                .1,
            Some(MouseEvent::Release {
                button: Button::Left,
                x: 70,
                y: 80
            })
        );
    }
    #[test]
    fn empty_modifier_never_swallows() {
        let mut m = MouseMatcher::default();
        let mut s = snap();
        s.modifier = Some(Modifiers::default());
        m.set_snapshot(s);
        assert!(matches!(
            m.on_button(Button::Left, true, 150, 150, Modifiers::default())
                .0,
            MouseAction::Pass
        ));
    }
    #[test]
    fn unmanaged_foreground_passes_press_but_not_running_drag() {
        let mut m = MouseMatcher::default();
        let mut s = snap();
        s.foreground_managed = false;
        m.set_snapshot(s);
        assert!(matches!(
            m.on_button(Button::Left, true, 150, 150, Modifiers::ALT),
            (MouseAction::Pass, None)
        ));
        assert!(!m.is_dragging());
        // A drag started while the foreground was managed keeps going.
        m.set_foreground_managed(true);
        m.on_button(Button::Left, true, 150, 150, Modifiers::ALT);
        m.set_foreground_managed(false);
        assert!(m.on_move(155, 155).is_some());
        assert!(matches!(
            m.on_button(Button::Left, false, 155, 155, Modifiers::default())
                .0,
            MouseAction::Swallow
        ));
    }
    #[test]
    fn menu_mask_only_for_alt_or_win() {
        assert!(needs_menu_mask(Modifiers::ALT));
        assert!(needs_menu_mask(Modifiers::WIN));
        assert!(needs_menu_mask(Modifiers::WIN | Modifiers::SHIFT));
        assert!(!needs_menu_mask(Modifiers::CTRL));
        assert!(!needs_menu_mask(Modifiers::CTRL | Modifiers::SHIFT));
        assert!(!needs_menu_mask(Modifiers::default()));
    }
    #[test]
    fn force_release_ends_drag_once() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        assert_eq!(m.force_release(0, 0), None);
        m.on_button(Button::Right, true, 50, 50, Modifiers::ALT);
        assert_eq!(
            m.force_release(70, 80),
            Some(MouseEvent::Release {
                button: Button::Right,
                x: 70,
                y: 80
            })
        );
        assert!(!m.is_dragging());
        assert_eq!(m.on_move(71, 81), None);
        // The late real button-up now passes.
        assert!(matches!(
            m.on_button(Button::Right, false, 71, 81, Modifiers::default())
                .0,
            MouseAction::Pass
        ));
    }
    #[test]
    fn moves_posted_only_when_wanted() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        assert_eq!(m.on_move(1, 1), None);
        let mut s = snap();
        s.want_moves = true;
        m.set_snapshot(s);
        assert_eq!(m.on_move(1, 1), Some(MouseEvent::Move { x: 1, y: 1 }));
    }
}
