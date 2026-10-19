//! Windows virtual desktop membership through the documented `IVirtualDesktopManager`.
//! Used to tell a window that was cloaked because the user switched desktops from one
//! that was cloaked for another reason.

use std::cell::RefCell;
use std::ffi::c_void;

use i3win_core::tree::WindowId;
use windows::core::Result;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};

thread_local! {
    /// Main-thread manager, created on first use and re-created after a failure.
    static MANAGER: RefCell<Option<IVirtualDesktopManager>> = const { RefCell::new(None) };
}

fn query(id: WindowId) -> Result<bool> {
    MANAGER.with(|m| {
        let mut slot = m.borrow_mut();
        if slot.is_none() {
            // SAFETY: plain COM call; COM is initialized on the main thread.
            *slot = Some(unsafe {
                CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_INPROC_SERVER)?
            });
        }
        let mgr = slot.as_ref().expect("manager just created");
        // SAFETY: plain COM call with a window handle value.
        let r = unsafe { mgr.IsWindowOnCurrentVirtualDesktop(HWND(id.0 as *mut c_void)) };
        match r {
            Ok(b) => Ok(b.as_bool()),
            Err(e) => {
                *slot = None;
                Err(e)
            }
        }
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
