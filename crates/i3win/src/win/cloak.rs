//! Hiding windows by DWM cloaking, like komorebi and GlazeWM. A cloaked window keeps
//! its surface, so it returns instantly instead of repainting like an `SW_HIDE` one.
//!
//! Cloaking another process's window needs the shell's undocumented
//! `IApplicationView::SetCloak`. The interface IIDs below come from the Windows 10
//! shell as used by komorebi, GlazeWM and VirtualDesktopAccessor; they may change
//! across Windows builds. Every failure is reported to the caller as `false`, which
//! falls back to the `SW_HIDE` path. Cloaked windows keep their taskbar button, so
//! the documented `ITaskbarList` removes and restores it to match the old behaviour.
// COM method names mirror the shell's vtable.
#![allow(non_snake_case)]

use std::cell::RefCell;
use std::ffi::c_void;

use i3win_core::tree::WindowId;
use windows::core::{Error, IUnknown, IUnknown_Vtbl, Interface, Result, GUID, HRESULT};
use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE, RPC_E_DISCONNECTED};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IServiceProvider, CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};

const CLSID_IMMERSIVE_SHELL: GUID = GUID::from_u128(0xC2F03A33_21F5_47FA_B4BB_156362A2F239);
const IID_APPLICATION_VIEW_COLLECTION: GUID =
    GUID::from_u128(0x1841C6D7_4F9D_42C0_AF41_8747538F10E5);
/// RPC_S_SERVER_UNAVAILABLE as an HRESULT (HRESULT_FROM_WIN32(1722)).
const RPC_S_SERVER_UNAVAILABLE_HR: HRESULT = HRESULT(0x8007_06BA_u32 as i32);

/// Only the vtable prefix up to `GetViewForHwnd` is declared; the first three take raw pointers.
#[windows::core::interface("1841C6D7-4F9D-42C0-AF41-8747538F10E5")]
unsafe trait IApplicationViewCollection: IUnknown {
    unsafe fn GetViews(&self, out: *mut *mut c_void) -> HRESULT;
    unsafe fn GetViewsByZOrder(&self, out: *mut *mut c_void) -> HRESULT;
    unsafe fn GetViewsByAppUserModelId(&self, id: *const u16, out: *mut *mut c_void) -> HRESULT;
    unsafe fn GetViewForHwnd(&self, hwnd: HWND, out: *mut *mut c_void) -> HRESULT;
}

/// Only the vtable prefix up to `SetCloak` is declared. It derives from IInspectable;
/// the macro cannot name that parent in windows 0.58, so its three methods are spelled out.
#[windows::core::interface("372E1D3B-38D3-42E4-A15B-8AB2B178F513")]
unsafe trait IApplicationView: IUnknown {
    unsafe fn GetIids(&self, count: *mut u32, iids: *mut *mut GUID) -> HRESULT;
    unsafe fn GetRuntimeClassName(&self, name: *mut *mut c_void) -> HRESULT;
    unsafe fn GetTrustLevel(&self, level: *mut i32) -> HRESULT;
    unsafe fn SetFocus(&self) -> HRESULT;
    unsafe fn SwitchTo(&self) -> HRESULT;
    unsafe fn TryInvokeBack(&self, callback: *mut c_void) -> HRESULT;
    unsafe fn GetThumbnailWindow(&self, out: *mut HWND) -> HRESULT;
    unsafe fn GetMonitor(&self, out: *mut *mut c_void) -> HRESULT;
    unsafe fn GetVisibility(&self, out: *mut i32) -> HRESULT;
    unsafe fn SetCloak(&self, cloak_type: i32, flags: i32) -> HRESULT;
}

struct Conn {
    views: IApplicationViewCollection,
    taskbar: ITaskbarList,
}

thread_local! {
    /// Main-thread connection to the shell, created on first use.
    static CONN: RefCell<Option<Conn>> = const { RefCell::new(None) };
}

/// Initializes COM on the current thread as an STA. Tolerates an already initialized
/// thread (S_FALSE) and one initialized with another model.
pub fn init_com() {
    // SAFETY: plain COM call; the thread keeps the apartment for its lifetime.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() && hr != RPC_E_CHANGED_MODE {
        log::warn!("CoInitializeEx failed: {hr:?}");
    }
}

fn connect() -> Result<Conn> {
    // SAFETY: COM calls with valid arguments; the out pointer outlives the call.
    unsafe {
        let sp: IServiceProvider =
            CoCreateInstance(&CLSID_IMMERSIVE_SHELL, None, CLSCTX_LOCAL_SERVER)?;
        let views: IApplicationViewCollection =
            sp.QueryService(&IID_APPLICATION_VIEW_COLLECTION)?;
        let taskbar: ITaskbarList = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
        taskbar.HrInit()?;
        Ok(Conn { views, taskbar })
    }
}

fn view_for(conn: &Conn, id: WindowId) -> Result<IApplicationView> {
    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: `raw` is a valid out pointer; on success it owns one reference.
    unsafe {
        conn.views
            .GetViewForHwnd(HWND(id.0 as *mut c_void), &mut raw)
            .ok()?;
        if raw.is_null() {
            return Err(Error::from(windows::Win32::Foundation::E_POINTER));
        }
        Ok(IApplicationView::from_raw(raw))
    }
}

fn set_cloak(conn: &Conn, id: WindowId, cloak: bool) -> Result<()> {
    let view = view_for(conn, id)?;
    // SAFETY: (1, 2) cloaks and (1, 0) uncloaks, as used by other window managers.
    unsafe { view.SetCloak(1, if cloak { 2 } else { 0 }).ok() }
}

fn is_disconnect(e: &Error) -> bool {
    let c = e.code();
    c == RPC_E_DISCONNECTED || c == RPC_S_SERVER_UNAVAILABLE_HR
}

/// Runs `f` on the cached connection; after an Explorer restart drops it and retries once.
fn with_conn(f: impl Fn(&Conn) -> Result<()>) -> Result<()> {
    CONN.with(|c| {
        let mut slot = c.borrow_mut();
        for attempt in 0..2 {
            if slot.is_none() {
                *slot = Some(connect()?);
            }
            match f(slot.as_ref().expect("connection just created")) {
                Err(e) if attempt == 0 && is_disconnect(&e) => {
                    log::debug!("cloak: shell connection lost, reconnecting");
                    *slot = None;
                }
                r => return r,
            }
        }
        unreachable!("second attempt always returns")
    })
}

/// Cloaks `id` and removes its taskbar button. False means nothing was cloaked.
pub fn cloak(id: WindowId) -> bool {
    let r = with_conn(|conn| {
        set_cloak(conn, id, true)?;
        // SAFETY: plain COM call.
        if let Err(e) = unsafe { conn.taskbar.DeleteTab(HWND(id.0 as *mut c_void)) } {
            log::debug!("cloak: DeleteTab {:#x}: {e}", id.0);
        }
        Ok(())
    });
    if let Err(e) = &r {
        log::debug!("cloak {:#x} failed: {e}", id.0);
    }
    r.is_ok()
}

/// Uncloaks `id` and restores its taskbar button. False means it may still be cloaked.
pub fn uncloak(id: WindowId) -> bool {
    let r = with_conn(|conn| {
        set_cloak(conn, id, false)?;
        // SAFETY: plain COM call.
        if let Err(e) = unsafe { conn.taskbar.AddTab(HWND(id.0 as *mut c_void)) } {
            log::debug!("cloak: AddTab {:#x}: {e}", id.0);
        }
        Ok(())
    });
    if let Err(e) = &r {
        log::debug!("uncloak {:#x} failed: {e}", id.0);
    }
    r.is_ok()
}

/// Removes the taskbar button of an already cloaked window.
pub fn remove_tab(id: WindowId) -> bool {
    let r = with_conn(|conn| {
        // SAFETY: plain COM call.
        unsafe { conn.taskbar.DeleteTab(HWND(id.0 as *mut c_void)) }
    });
    if let Err(e) = &r {
        log::debug!("remove_tab {:#x} failed: {e}", id.0);
    }
    r.is_ok()
}

/// Drops the cached shell connection (Explorer restarted); the next call reconnects.
pub fn reset() {
    CONN.with(|c| *c.borrow_mut() = None);
}

/// Uncloaks `id` from any thread (crash and exit paths) with its own COM objects,
/// leaving the main-thread cache alone. Harmless for a window that is not cloaked.
pub fn uncloak_fresh(id: WindowId) {
    init_com();
    let r = connect().and_then(|conn| {
        set_cloak(&conn, id, false)?;
        // SAFETY: plain COM call.
        let _ = unsafe { conn.taskbar.AddTab(HWND(id.0 as *mut c_void)) };
        Ok(())
    });
    if let Err(e) = r {
        log::debug!("uncloak_fresh {:#x}: {e}", id.0);
    }
}
