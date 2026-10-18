//! Low-level mouse hook plus a pure matcher that decides which buttons start a drag.
//!
//! The hook runs on the main thread and only reads the thread-local snapshot the main
//! thread writes each flush: no allocation in the common path, no cross-process calls.
use std::cell::{Cell, RefCell};

use i3win_core::geom::Rect;
use i3win_core::keys::Modifiers;
use i3win_core::tree::{NodeId, WindowId};
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
/// Events that fit before the main thread drains, reserved up front. The queue
/// never grows past it (the hook must not allocate): when full, events are dropped.
const QUEUE_CAPACITY: usize = 256;

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
    /// A left-button drag on a title bar or tab passed the movement threshold;
    /// (x, y) is where the button went down. Its moves and release follow as
    /// `Move` and `Release { button: Left, .. }`.
    TitlePress {
        x: i32,
        y: i32,
        node: NodeId,
    },
    /// Escape during a drag: undo it.
    Cancel,
}

#[derive(Debug)]
pub enum MouseAction {
    Pass,
    Swallow,
}

#[derive(Default)]
pub struct MouseMatcher {
    snapshot: Snapshot,
    /// The button whose press this matcher swallowed, until that button's release
    /// (which is swallowed too, so the application never sees an unpaired button).
    swallowed: Option<Button>,
    /// The swallowed press's drag still runs in the App: moves and its release are
    /// posted. False after an App-side cancel (Escape).
    drag_live: bool,
    /// Release of a drag whose button-up was missed, found at the next press of that
    /// button; the hook queues it before that press's own event.
    missed_release: Option<MouseEvent>,
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
            if self.swallowed == Some(button) {
                self.swallowed = None;
                let live = std::mem::take(&mut self.drag_live);
                return (
                    MouseAction::Swallow,
                    live.then_some(MouseEvent::Release { button, x, y }),
                );
            }
            return (MouseAction::Pass, None);
        }
        if self.swallowed == Some(button) {
            // A second press of the swallowed button: its release was missed. End
            // that drag (the App gets its Release first) and judge this press afresh.
            self.swallowed = None;
            if std::mem::take(&mut self.drag_live) {
                self.missed_release = Some(MouseEvent::Release { button, x, y });
            }
        }
        // One drag at a time; Middle never starts one; nothing over an unmanaged foreground.
        if self.swallowed.is_some() || button == Button::Middle || !self.snapshot.foreground_managed
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
                self.swallowed = Some(button);
                self.drag_live = true;
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

    /// The Release of a drag whose button-up was missed, found by the last press.
    pub fn take_missed_release(&mut self) -> Option<MouseEvent> {
        self.missed_release.take()
    }

    /// `Some(Move)` iff a drag is in progress or the snapshot wants moves.
    pub fn on_move(&mut self, x: i32, y: i32) -> Option<MouseEvent> {
        (self.drag_live || self.snapshot.want_moves).then_some(MouseEvent::Move { x, y })
    }

    /// A press this matcher swallowed is still held (its release not seen yet).
    pub fn holds_swallowed(&self) -> bool {
        self.swallowed.is_some()
    }

    #[cfg(test)]
    pub fn is_dragging(&self) -> bool {
        self.drag_live
    }

    /// Updates only the foreground flag of the snapshot (Foreground events).
    pub fn set_foreground_managed(&mut self, managed: bool) {
        self.snapshot.foreground_managed = managed;
    }

    /// Ends a drag whose button-up was missed (the App saw the button no longer down):
    /// clears the drag and returns the Release at (x, y); `None` without a drag.
    pub fn force_release(&mut self, x: i32, y: i32) -> Option<MouseEvent> {
        let button = self.swallowed.take()?;
        std::mem::take(&mut self.drag_live).then_some(MouseEvent::Release { button, x, y })
    }

    /// The App cancelled the drag (Escape): stop posting its moves and release, but
    /// keep swallowing the held button's release, the pair of the swallowed press.
    pub fn cancel_drag(&mut self) {
        self.drag_live = false;
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
    /// The App has a drag in progress (set by the App; read by the keyboard hook).
    static DRAG_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Diagnostic: how many times the hook proc ran.
    static HOOK_CALLS: Cell<u64> = const { Cell::new(0) };
}

/// Diagnostic: hook proc invocations so far.
pub fn hook_calls() -> u64 {
    HOOK_CALLS.with(|c| c.get())
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

/// Pushes `ev` after the move it followed, never past `cap` (no reallocation):
/// the move is dropped first when only `ev` fits; false when `ev` does not fit.
fn push_bounded(
    q: &mut Vec<MouseEvent>,
    cap: usize,
    latest_move: Option<(i32, i32)>,
    ev: MouseEvent,
) -> bool {
    if let Some((x, y)) = latest_move {
        if q.len() + 2 <= cap {
            q.push(MouseEvent::Move { x, y });
        }
    }
    if q.len() < cap {
        q.push(ev);
        true
    } else {
        false
    }
}

/// Queues an event after the move it followed; false if the queue is busy or full.
fn queue(ev: MouseEvent) -> bool {
    QUEUE.with(|q| match q.try_borrow_mut() {
        Ok(mut q) => {
            let latest = LATEST_MOVE.with(|m| m.take());
            push_bounded(&mut q, QUEUE_CAPACITY, latest, ev)
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
    HOOK_CALLS.with(|c| c.set(c.get().wrapping_add(1)));
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
                keyboard::held_mods_resynced()
            } else {
                Modifiers::default()
            };
            let (action, ev, missed) = MATCHER.with(|m| match m.try_borrow_mut() {
                Ok(mut m) => {
                    let (action, ev) = m.on_button(button, down, x, y, mods);
                    (action, ev, m.take_missed_release())
                }
                Err(_) => (MouseAction::Pass, None, None),
            });
            if let Some(r) = missed {
                if queue(r) {
                    wake();
                }
            }
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

/// Main thread: the App started (true) or ended (false) a drag.
pub fn set_drag_active(active: bool) {
    DRAG_ACTIVE.with(|d| d.set(active));
}

/// Whether the App has a drag in progress. Allocation-free (keyboard hook).
pub fn is_dragging() -> bool {
    DRAG_ACTIVE.with(|d| d.get())
}

/// Main thread: the App cancelled a hook drag; see `MouseMatcher::cancel_drag`.
pub fn cancel_drag() {
    MATCHER.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.cancel_drag();
        }
    });
}

/// Whether a press the hook swallowed is still held: the async button state never
/// shows it down, so hover must ask here. Allocation-free.
pub fn holds_swallowed_press() -> bool {
    MATCHER.with(|m| m.try_borrow().is_ok_and(|m| m.holds_swallowed()))
}

/// Main thread (deco windows, keyboard hook): queues an event into the same path
/// as the mouse hook's and wakes the main window.
pub fn post_event(ev: MouseEvent) {
    if queue(ev) {
        wake();
    }
}

/// Main thread (captured title-bar drag): a pointer move, coalesced like the hook's.
pub fn post_move(x: i32, y: i32) {
    LATEST_MOVE.with(|m| m.set(Some((x, y))));
    wake();
}

/// Ends a hook drag whose button-up was missed; see `MouseMatcher::force_release`.
pub fn force_release(x: i32, y: i32) -> Option<MouseEvent> {
    MATCHER.with(|m| {
        m.try_borrow_mut()
            .ok()
            .and_then(|mut m| m.force_release(x, y))
    })
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
    fn queue_never_grows_past_capacity() {
        let mut q = Vec::with_capacity(3);
        let ev = MouseEvent::Cancel;
        assert!(push_bounded(&mut q, 3, Some((1, 1)), ev));
        assert_eq!(q, vec![MouseEvent::Move { x: 1, y: 1 }, ev]);
        // Only one slot left: the move is dropped, the event kept.
        assert!(push_bounded(&mut q, 3, Some((2, 2)), ev));
        assert_eq!(q.len(), 3);
        assert_eq!(q[2], ev);
        assert!(!push_bounded(&mut q, 3, None, ev));
        assert_eq!((q.len(), q.capacity()), (3, 3));
    }
    #[test]
    fn stuck_alt_resynced_up_lets_press_pass() {
        use crate::win::keyboard::KeyMatcher;
        let mut k = KeyMatcher::new();
        k.on_key(0xA4, true, false, 0, false);
        // Alt's key-up went to the secure desktop: the tracked state is stale.
        assert_eq!(k.mods(), Modifiers::ALT);
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        let mods = k.resynced_mods(|_| false);
        assert_eq!(mods, Modifiers::default());
        assert!(matches!(
            m.on_button(Button::Left, true, 150, 150, mods),
            (MouseAction::Pass, None)
        ));
        // Alt really held: the async state confirms it and the press is swallowed.
        k.on_key(0xA4, true, false, 0, false);
        let mods = k.resynced_mods(|vk| vk == 0xA4);
        assert!(matches!(
            m.on_button(Button::Left, true, 150, 150, mods).0,
            MouseAction::Swallow
        ));
    }
    #[test]
    fn missed_release_is_found_at_next_press() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        m.on_button(Button::Left, true, 150, 150, Modifiers::ALT);
        assert_eq!(m.take_missed_release(), None);
        // The hook never saw the release; a plain press of the same button follows.
        let (a, ev) = m.on_button(Button::Left, true, 300, 300, Modifiers::default());
        assert!(matches!(a, MouseAction::Pass));
        assert_eq!(ev, None);
        assert_eq!(
            m.take_missed_release(),
            Some(MouseEvent::Release {
                button: Button::Left,
                x: 300,
                y: 300
            })
        );
        assert!(!m.is_dragging());
        assert!(!m.holds_swallowed());
        // That press passed, so its release passes too: the pair stays intact.
        assert!(matches!(
            m.on_button(Button::Left, false, 300, 300, Modifiers::default()),
            (MouseAction::Pass, None)
        ));
        // A modifier press after the missed release starts a new drag at once.
        m.on_button(Button::Right, true, 150, 150, Modifiers::ALT);
        let (a, ev) = m.on_button(Button::Right, true, 160, 160, Modifiers::ALT);
        assert!(matches!(a, MouseAction::Swallow));
        assert!(matches!(ev, Some(MouseEvent::Press { .. })));
        assert!(m.take_missed_release().is_some());
        assert!(m.is_dragging());
    }
    #[test]
    fn cancel_keeps_the_release_swallowed() {
        let mut m = MouseMatcher::default();
        m.set_snapshot(snap());
        m.on_button(Button::Left, true, 150, 150, Modifiers::ALT);
        assert!(m.holds_swallowed());
        m.cancel_drag();
        assert!(!m.is_dragging());
        assert_eq!(m.on_move(160, 160), None);
        assert!(m.holds_swallowed());
        // The release pairs with the swallowed press: swallowed, but no Release event.
        assert!(matches!(
            m.on_button(Button::Left, false, 160, 160, Modifiers::default()),
            (MouseAction::Swallow, None)
        ));
        assert!(!m.holds_swallowed());
        // Missed release after a cancel: the next press is judged afresh, no Release.
        m.on_button(Button::Left, true, 150, 150, Modifiers::ALT);
        m.cancel_drag();
        assert!(matches!(
            m.on_button(Button::Left, true, 150, 150, Modifiers::default()),
            (MouseAction::Pass, None)
        ));
        assert_eq!(m.take_missed_release(), None);
        assert!(!m.holds_swallowed());
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
