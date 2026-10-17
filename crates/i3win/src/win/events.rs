//! WinEvent hook: translates raw events into a thread-local queue drained by the main loop.
use std::cell::RefCell;

use i3win_core::tree::WindowId;
use windows::core::{Error, Result};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};

use crate::mainloop::{main_hwnd, WM_APP_EVENTS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformEvent {
    Shown(WindowId),
    Hidden(WindowId),
    Destroyed(WindowId),
    Foreground(WindowId),
    Minimized(WindowId),
    Restored(WindowId),
    MoveSizeStart(WindowId),
    MoveSizeEnd(WindowId),
    TitleChanged(WindowId),
}

pub fn translate(event: u32, hwnd: isize, id_object: i32, id_child: i32) -> Option<PlatformEvent> {
    if hwnd == 0 || id_object != 0 || id_child != 0 {
        return None;
    }
    let id = WindowId(hwnd);
    match event {
        0x8002 | 0x8018 => Some(PlatformEvent::Shown(id)),
        0x8003 | 0x8017 => Some(PlatformEvent::Hidden(id)),
        0x8001 => Some(PlatformEvent::Destroyed(id)),
        0x0003 => Some(PlatformEvent::Foreground(id)),
        0x0016 => Some(PlatformEvent::Minimized(id)),
        0x0017 => Some(PlatformEvent::Restored(id)),
        0x000A => Some(PlatformEvent::MoveSizeStart(id)),
        0x000B => Some(PlatformEvent::MoveSizeEnd(id)),
        0x800C => Some(PlatformEvent::TitleChanged(id)),
        _ => None,
    }
}

thread_local! {
    static QUEUE: RefCell<Vec<PlatformEvent>> = const { RefCell::new(Vec::new()) };
    static HOOKS: RefCell<Vec<HWINEVENTHOOK>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn callback(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let Some(ev) = translate(event, hwnd.0 as isize, id_object, id_child) else {
        return;
    };
    // try_borrow_mut: never panic inside a hook callback; skip if busy.
    let first = QUEUE.with(|q| match q.try_borrow_mut() {
        Ok(mut q) => {
            let was_empty = q.is_empty();
            q.push(ev);
            was_empty
        }
        Err(_) => false,
    });
    if first {
        let main = main_hwnd();
        if main != 0 {
            let _ = PostMessageW(HWND(main as *mut _), WM_APP_EVENTS, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn install() -> Result<()> {
    let ranges = [(0x0003u32, 0x0017u32), (0x8001u32, 0x8018u32)];
    for (min, max) in ranges {
        let h = unsafe {
            SetWinEventHook(
                min,
                max,
                None,
                Some(callback),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            )
        };
        if h.is_invalid() {
            uninstall();
            return Err(Error::from_win32());
        }
        HOOKS.with(|hs| hs.borrow_mut().push(h));
    }
    Ok(())
}

pub fn uninstall() {
    let hooks: Vec<HWINEVENTHOOK> = HOOKS.with(|hs| hs.borrow_mut().drain(..).collect());
    for h in hooks {
        unsafe {
            let _ = UnhookWinEvent(h);
        }
    }
}

pub fn drain() -> Vec<PlatformEvent> {
    QUEUE.with(|q| q.borrow_mut().drain(..).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_known_events() {
        assert_eq!(
            translate(0x8002, 7, 0, 0),
            Some(PlatformEvent::Shown(WindowId(7)))
        );
        assert_eq!(
            translate(0x8017, 7, 0, 0),
            Some(PlatformEvent::Hidden(WindowId(7)))
        );
        assert_eq!(
            translate(0x0003, 7, 0, 0),
            Some(PlatformEvent::Foreground(WindowId(7)))
        );
        assert_eq!(
            translate(0x800C, 7, 0, 0),
            Some(PlatformEvent::TitleChanged(WindowId(7)))
        );
    }

    #[test]
    fn translates_move_size_start_and_end() {
        assert_eq!(
            translate(0x000A, 7, 0, 0),
            Some(PlatformEvent::MoveSizeStart(WindowId(7)))
        );
        assert_eq!(
            translate(0x000B, 7, 0, 0),
            Some(PlatformEvent::MoveSizeEnd(WindowId(7)))
        );
    }

    #[test]
    fn ignores_children_and_noise() {
        assert_eq!(translate(0x8002, 7, -4, 0), None);
        assert_eq!(translate(0x8002, 7, 0, 3), None);
        assert_eq!(translate(0x800B, 7, 0, 0), None);
        assert_eq!(translate(0x8002, 0, 0, 0), None);
    }
}
