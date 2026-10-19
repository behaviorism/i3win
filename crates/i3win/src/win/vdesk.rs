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
//!
//! That leaves a gap: leaving desktop G from an empty i3 workspace parks nothing, and
//! a new desktop without windows gives no `current_desktop_hint`. So i3win owns a
//! desktop marker: a 1x1 `WS_POPUP` owned by the main window (no taskbar button, so
//! it keeps a desktop association) and cloaked with `DWMWA_CLOAK` (documented for
//! one's own windows). Verified: `GetWindowDesktopId` returns the desktop it was
//! shown on, `IsWindowOnCurrentVirtualDesktop` is TRUE only there, and
//! `MoveWindowToDesktop` works on it. An unowned tool window (the main window) gets
//! GUID_NULL and cannot serve. The marker is moved to the desktop wherever the
//! current one is learned, so "marker is elsewhere" means "we have left its desktop".

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use i3win_core::tree::WindowId;
use std::collections::HashSet;

use windows::core::{w, Result, GUID};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_CLOAK, DWMWA_CLOAKED,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, EnumWindows, GetWindowThreadProcessId,
    IsWindowVisible, RegisterClassW, ShowWindow, SW_SHOWNA, WNDCLASSW, WS_EX_NOACTIVATE, WS_POPUP,
};

thread_local! {
    /// The desktop marker window (0: none), see the module docs.
    static MARKER: Cell<isize> = const { Cell::new(0) };
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

unsafe extern "system" fn marker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: plain default handling.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Creates the desktop marker on the current desktop, owned by the main window
/// `owner`. Call once after the main window exists.
pub fn create_marker(owner: isize) {
    if owner == 0 || MARKER.get() != 0 {
        return;
    }
    // SAFETY: registers and creates a window of our own class on the UI thread.
    let hwnd = unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else {
            return;
        };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(marker_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("i3win_marker"),
            ..Default::default()
        };
        // Fails when already registered, which is fine.
        RegisterClassW(&wc);
        // On-screen (off-screen windows may be treated differently), never activated.
        match CreateWindowExW(
            WS_EX_NOACTIVATE,
            w!("i3win_marker"),
            w!("i3win desktop marker"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            HWND(owner as *mut c_void),
            None,
            hinst,
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("vdesk: marker window: {e}");
                return;
            }
        }
    };
    // SAFETY: plain calls on our own window; it never paints before being cloaked.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let on = BOOL(1);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            &on as *const BOOL as *const c_void,
            std::mem::size_of::<BOOL>() as u32,
        );
    }
    MARKER.set(hwnd.0 as isize);
}

/// Destroys the marker (shutdown).
pub fn destroy_marker() {
    let m = MARKER.replace(0);
    if m != 0 {
        // SAFETY: our own window, destroyed on its creating thread.
        let _ = unsafe { DestroyWindow(HWND(m as *mut c_void)) };
    }
}

/// The marker's desktop and whether that is the current one; None when there is no
/// marker or the queries fail.
pub fn marker_state() -> Option<(GUID, bool)> {
    let m = WindowId(MARKER.get());
    if m.0 == 0 {
        return None;
    }
    Some((desktop_of(m)?, on_current_desktop(m)?))
}

/// Moves the marker to `desktop`.
pub fn move_marker_to(desktop: GUID) {
    let m = MARKER.get();
    if m == 0 {
        return;
    }
    if let Err(e) = with_manager(|mgr| {
        // SAFETY: plain COM call with a window handle value.
        unsafe { mgr.MoveWindowToDesktop(HWND(m as *mut c_void), &desktop) }
    }) {
        log::debug!("vdesk: moving the marker: {e}");
    }
}

/// The current-desktop hint plus the marker state. A hint pulls the marker along.
pub fn current_context() -> (Option<GUID>, Option<(GUID, bool)>) {
    let hint = current_desktop_hint();
    if let Some(cur) = hint {
        if marker_state().is_some_and(|(_, on)| !on) {
            move_marker_to(cur);
        }
    }
    (hint, marker_state())
}

/// The current desktop if known: the hint, else the marker's when it is on current.
fn known_current(hint: Option<GUID>, marker: Option<(GUID, bool)>) -> Option<GUID> {
    hint.or(match marker {
        Some((g, true)) => Some(g),
        _ => None,
    })
}

/// Whether a window i3win hid (desktop `recorded`, unqueryable now) is off the current
/// desktop, and the desktop to mark as left, if any. When the current desktop is
/// unknown but the marker says we left its desktop, that desktop is not the current
/// one either. Unknown otherwise means "allow".
pub fn should_park_hidden(
    recorded: GUID,
    away: &HashSet<GUID>,
    hint: Option<GUID>,
    marker: Option<(GUID, bool)>,
) -> (bool, Option<GUID>) {
    if away.contains(&recorded) {
        return (true, None);
    }
    match known_current(hint, marker) {
        Some(cur) if cur != recorded => (true, Some(recorded)),
        Some(_) => (false, None),
        None if matches!(marker, Some((g, false)) if g == recorded) => (true, Some(recorded)),
        None => (false, None),
    }
}

/// Whether a parked i3win-hidden window with desktop `recorded` may come back: its
/// desktop is the current one, or is not marked away (a returning sibling cleared it)
/// and nothing says we are elsewhere.
pub fn sweep_unparks_hidden(
    recorded: GUID,
    away: &HashSet<GUID>,
    hint: Option<GUID>,
    marker: Option<(GUID, bool)>,
) -> bool {
    let cur = known_current(hint, marker);
    if cur == Some(recorded) {
        return true;
    }
    let elsewhere = match cur {
        Some(_) => true,
        None => matches!(marker, Some((g, false)) if g == recorded),
    };
    !away.contains(&recorded) && !elsewhere
}

#[cfg(test)]
mod tests {
    use super::*;

    const D1: GUID = GUID::from_u128(1);
    const D2: GUID = GUID::from_u128(2);

    #[test]
    fn recorded_desktop_away_parks() {
        let away = HashSet::from([D1]);
        assert_eq!(should_park_hidden(D1, &away, None, None), (true, None));
        assert_eq!(should_park_hidden(D1, &away, Some(D2), None), (true, None));
    }

    #[test]
    fn differing_hint_marks_away() {
        let away = HashSet::new();
        assert_eq!(
            should_park_hidden(D1, &away, Some(D2), None),
            (true, Some(D1))
        );
    }

    #[test]
    fn unknown_or_matching_hint_allows() {
        let away = HashSet::new();
        assert_eq!(should_park_hidden(D1, &away, None, None), (false, None));
        assert_eq!(should_park_hidden(D1, &away, Some(D1), None), (false, None));
    }

    #[test]
    fn return_clearing_away_stops_parking() {
        let mut away = HashSet::from([D1]);
        assert!(should_park_hidden(D1, &away, None, None).0);
        away.remove(&D1);
        assert!(!should_park_hidden(D1, &away, None, None).0);
    }

    #[test]
    fn marker_left_behind_parks_its_desktop_windows() {
        let away = HashSet::new();
        // The gap case: no hint, the marker stayed on D1 while we moved on.
        assert_eq!(
            should_park_hidden(D1, &away, None, Some((D1, false))),
            (true, Some(D1))
        );
        // A window of another desktop is not implicated.
        assert_eq!(
            should_park_hidden(D2, &away, None, Some((D1, false))),
            (false, None)
        );
    }

    #[test]
    fn marker_on_current_gives_the_current_desktop() {
        let away = HashSet::new();
        assert_eq!(
            should_park_hidden(D1, &away, None, Some((D1, true))),
            (false, None)
        );
        assert_eq!(
            should_park_hidden(D1, &away, None, Some((D2, true))),
            (true, Some(D1))
        );
    }

    #[test]
    fn sweep_keeps_hidden_windows_parked_while_away() {
        let away = HashSet::from([D1]);
        assert!(!sweep_unparks_hidden(D1, &away, None, None));
        assert!(!sweep_unparks_hidden(D1, &away, Some(D2), None));
        assert!(sweep_unparks_hidden(D1, &away, Some(D1), None));
        assert!(sweep_unparks_hidden(D1, &away, None, Some((D1, true))));
        assert!(!sweep_unparks_hidden(D1, &away, None, Some((D1, false))));
        assert!(sweep_unparks_hidden(D1, &HashSet::new(), None, None));
        assert!(!sweep_unparks_hidden(
            D1,
            &HashSet::new(),
            None,
            Some((D1, false))
        ));
    }
}
