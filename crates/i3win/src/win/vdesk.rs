//! Windows virtual desktop membership through the documented `IVirtualDesktopManager`.
//! Used to tell a window that was cloaked because the user switched desktops from one
//! that was cloaked for another reason.
//!
//! Verified on Windows 10 19045: `ITaskbarList::DeleteTab` (which i3win calls on every
//! window it hides) erases the window's desktop association. Afterwards
//! `GetWindowDesktopId` returns GUID_NULL and `IsWindowOnCurrentVirtualDesktop` is TRUE
//! on every desktop. So the app records `desktop_of` just before hiding a window
//! (`hidden_desktop`) and reasons about the desktops it has left itself; the plain
//! queries here only answer for windows that still have their tab.

use std::cell::RefCell;
use std::ffi::c_void;

use i3win_core::tree::WindowId;
use std::collections::HashSet;

use windows::core::{Result, GUID};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};

thread_local! {
    /// Main-thread manager, created on first use and re-created after a failure.
    static MANAGER: RefCell<Option<IVirtualDesktopManager>> = const { RefCell::new(None) };
}

fn with_manager<T>(f: impl FnOnce(&IVirtualDesktopManager) -> Result<T>) -> Result<T> {
    MANAGER.with(|m| {
        let mut slot = m.borrow_mut();
        if slot.is_none() {
            // SAFETY: plain COM call; COM is initialized on the main thread.
            *slot = Some(unsafe {
                CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_INPROC_SERVER)?
            });
        }
        let mgr = slot.as_ref().expect("manager just created");
        let r = f(mgr);
        if r.is_err() {
            *slot = None;
        }
        r
    })
}

fn query(id: WindowId) -> Result<bool> {
    with_manager(|mgr| {
        // SAFETY: plain COM call with a window handle value.
        unsafe { mgr.IsWindowOnCurrentVirtualDesktop(HWND(id.0 as *mut c_void)) }
            .map(|b| b.as_bool())
    })
}

/// Whether `id` is on the current virtual desktop; None when that cannot be told.
pub fn on_current_desktop(id: WindowId) -> Option<bool> {
    match query(id) {
        Ok(b) => Some(b),
        Err(e) => {
            log::debug!("vdesk: {:#x}: {e}", id.0);
            None
        }
    }
}

/// The virtual desktop `id` belongs to; None on error or GUID_NULL (a window whose
/// tab was deleted has no association).
pub fn desktop_of(id: WindowId) -> Option<GUID> {
    match with_manager(|mgr| {
        // SAFETY: plain COM call with a window handle value.
        unsafe { mgr.GetWindowDesktopId(HWND(id.0 as *mut c_void)) }
    }) {
        Ok(g) if g != GUID::zeroed() => Some(g),
        Ok(_) => None,
        Err(e) => {
            log::debug!("vdesk: {:#x}: {e}", id.0);
            None
        }
    }
}

unsafe extern "system" fn hint_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the `Option<GUID>` slot `current_desktop_hint` passes.
    let out = unsafe { &mut *(lparam.0 as *mut Option<GUID>) };
    // SAFETY: plain queries on a window handle value.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
        .is_err()
            || cloaked != 0
        {
            return BOOL(1);
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return BOOL(1);
        }
    }
    match desktop_of(WindowId(hwnd.0 as isize)) {
        Some(g) => {
            *out = Some(g);
            BOOL(0)
        }
        None => BOOL(1),
    }
}

/// The current virtual desktop, inferred from the first visible, uncloaked window of
/// another process that has a desktop (windows on other desktops are cloaked). None
/// when there is none, e.g. on an empty new desktop.
pub fn current_desktop_hint() -> Option<GUID> {
    let mut out: Option<GUID> = None;
    // SAFETY: the callback only runs during this call and `out` outlives it.
    // EnumWindows reports an error when the callback stops early; that is expected.
    let _ = unsafe { EnumWindows(Some(hint_proc), LPARAM(&mut out as *mut _ as isize)) };
    out
}

/// Whether a window i3win hid (desktop `recorded`, unqueryable now) is off the current
/// desktop, and the desktop to mark as left, if any. Unknown means "allow".
pub fn should_park_hidden(
    recorded: GUID,
    away: &HashSet<GUID>,
    hint: Option<GUID>,
) -> (bool, Option<GUID>) {
    if away.contains(&recorded) {
        return (true, None);
    }
    match hint {
        Some(cur) if cur != recorded => (true, Some(recorded)),
        _ => (false, None),
    }
}

/// Whether a parked i3win-hidden window with desktop `recorded` may come back: its
/// desktop is not marked away (a returning sibling cleared it) or is the current one.
pub fn sweep_unparks_hidden(recorded: GUID, away: &HashSet<GUID>, hint: Option<GUID>) -> bool {
    !away.contains(&recorded) || hint == Some(recorded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D1: GUID = GUID::from_u128(1);
    const D2: GUID = GUID::from_u128(2);

    #[test]
    fn recorded_desktop_away_parks() {
        let away = HashSet::from([D1]);
        assert_eq!(should_park_hidden(D1, &away, None), (true, None));
        assert_eq!(should_park_hidden(D1, &away, Some(D2)), (true, None));
    }

    #[test]
    fn differing_hint_marks_away() {
        let away = HashSet::new();
        assert_eq!(should_park_hidden(D1, &away, Some(D2)), (true, Some(D1)));
    }

    #[test]
    fn unknown_or_matching_hint_allows() {
        let away = HashSet::new();
        assert_eq!(should_park_hidden(D1, &away, None), (false, None));
        assert_eq!(should_park_hidden(D1, &away, Some(D1)), (false, None));
    }

    #[test]
    fn return_clearing_away_stops_parking() {
        let mut away = HashSet::from([D1]);
        assert!(should_park_hidden(D1, &away, None).0);
        away.remove(&D1);
        assert!(!should_park_hidden(D1, &away, None).0);
    }

    #[test]
    fn sweep_keeps_hidden_windows_parked_while_away() {
        let away = HashSet::from([D1]);
        assert!(!sweep_unparks_hidden(D1, &away, None));
        assert!(!sweep_unparks_hidden(D1, &away, Some(D2)));
        assert!(sweep_unparks_hidden(D1, &away, Some(D1)));
        assert!(sweep_unparks_hidden(D1, &HashSet::new(), None));
    }
}
