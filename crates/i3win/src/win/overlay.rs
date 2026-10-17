//! The tiling-drag drop preview: a translucent, click-through, topmost tool window
//! that never activates or takes focus.
use std::cell::Cell;

use i3win_core::config::Color;
use i3win_core::geom::Rect;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::win::gdi;

const ALPHA: u8 = 128;

thread_local! {
    /// The fill color of the overlay window (one overlay per process).
    static FILL: Cell<Color> = const { Cell::new(Color(0x28, 0x55, 0x77)) };
}

#[derive(Default)]
pub struct Overlay {
    hwnd: Option<HWND>,
    shown: Option<(Rect, Color)>,
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let c = FILL.with(|f| f.get());
            gdi::paint_buffered(hwnd, |hdc, client| gdi::fill(hdc, client, c));
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn create() -> Option<HWND> {
    // SAFETY: registers and creates a window of our own class on the UI thread.
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(overlay_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("i3win_drop"),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            log::error!("registering i3win_drop failed");
            return None;
        }
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            w!("i3win_drop"),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            hinst,
            None,
        )
        .map_err(|e| log::error!("creating drop overlay failed: {e}"))
        .ok()?;
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), ALPHA, LWA_ALPHA);
        Some(hwnd)
    }
}

impl Overlay {
    /// Shows the preview over `r` filled with `color` (created on first use).
    pub fn show(&mut self, r: Rect, color: Color) {
        if self.shown == Some((r, color)) {
            return;
        }
        if self.hwnd.is_none() {
            self.hwnd = create();
        }
        let Some(hwnd) = self.hwnd else {
            return;
        };
        FILL.with(|f| f.set(color));
        // SAFETY: plain Win32 calls on our own window.
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                r.x,
                r.y,
                r.w.max(1),
                r.h.max(1),
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            let _ = InvalidateRect(hwnd, None, false);
        }
        self.shown = Some((r, color));
    }

    pub fn hide(&mut self) {
        if self.shown.take().is_none() {
            return;
        }
        if let Some(hwnd) = self.hwnd {
            // SAFETY: hiding our own window.
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    /// Destroys the overlay window (shutdown).
    pub fn destroy(&mut self) {
        self.shown = None;
        if let Some(hwnd) = self.hwnd.take() {
            // SAFETY: destroying our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }
}
