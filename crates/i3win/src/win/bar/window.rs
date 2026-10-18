//! Bar window class, creation, `BarData` storage and the window procedure.
use i3win_core::config::{BarBinding, Font};
use i3win_core::geom::Rect;
use i3win_core::i3bar::Block;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::input::{lparam_point, on_button, wheel_steps, HWHEEL_ACC, WHEEL_ACC};
use super::paint::paint;
use crate::bar_layout::BarLayout;
use crate::win::gdi;

/// Everything a bar window needs to paint and hit-test itself.
#[derive(Clone, Debug, PartialEq)]
pub struct BarData {
    /// Id of the configured bar this window belongs to (routes clicks).
    pub bar_id: String,
    pub layout: BarLayout,
    pub font: Font,
    pub scale: f32,
    pub blocks: Vec<Block>,
    /// The bar's `bindsym button<N>` entries, so the window proc needs no `App`.
    pub bindings: Vec<BarBinding>,
    /// Whether the bar's status process asked for click events.
    pub wants_clicks: bool,
    /// Screen origin of the bar's output (work-area) rect, for `output_x/y`.
    pub output_origin: (i32, i32),
}

pub(super) fn register_class() {
    // SAFETY: plain class registration; "already exists" is fine.
    unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else {
            return;
        };
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(bar_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("i3win_bar"),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            log::error!("registering i3win_bar failed");
        }
    }
}

pub(super) fn create_window(r: Rect, data: &BarData) -> Option<HWND> {
    // SAFETY: creates a window of our own class; ownership of the boxed data
    // passes to GWLP_USERDATA and is freed in WM_NCDESTROY.
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
            w!("i3win_bar"),
            PCWSTR::null(),
            WS_POPUP,
            r.x,
            r.y,
            r.w.max(1),
            r.h.max(1),
            None,
            None,
            hinst,
            None,
        )
        .map_err(|e| log::error!("creating bar window failed: {e}"))
        .ok()?;
        let ptr = Box::into_raw(Box::new(data.clone()));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        Some(hwnd)
    }
}

pub(super) fn set_data(hwnd: HWND, data: &BarData) {
    // SAFETY: the pointer is the live Box stored by `create_window`; we are on
    // the UI thread and no paint is in progress while this runs.
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut BarData;
        if let Some(slot) = p.as_mut() {
            *slot = data.clone();
        }
        let _ = InvalidateRect(hwnd, None, false);
    }
}
unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut BarData;
    match msg {
        WM_PAINT => {
            match ptr.as_ref() {
                Some(data) => paint(hwnd, data),
                // Still validate the window so WM_PAINT does not repeat.
                None => gdi::paint_buffered(hwnd, |_, _| {}),
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONUP | WM_MBUTTONUP
        | WM_RBUTTONUP => {
            let (button, release) = match msg {
                WM_LBUTTONDOWN => (1, false),
                WM_MBUTTONDOWN => (2, false),
                WM_RBUTTONDOWN => (3, false),
                WM_LBUTTONUP => (1, true),
                WM_MBUTTONUP => (2, true),
                _ => (3, true),
            };
            if let Some(data) = ptr.as_ref() {
                let (x, y) = lparam_point(lp);
                on_button(hwnd, data, button, release, x, y, 1);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let horizontal = msg == WM_MOUSEHWHEEL;
            let delta = ((wp.0 >> 16) & 0xffff) as i16 as i32;
            let acc_cell = if horizontal { &HWHEEL_ACC } else { &WHEEL_ACC };
            let steps = acc_cell.with(|a| {
                let mut acc = a.get();
                let n = wheel_steps(&mut acc, delta);
                a.set(acc);
                n
            });
            if steps == 0 {
                return LRESULT(0);
            }
            // i3 numbering: 4 up, 5 down, 6 left, 7 right.
            let button = match (horizontal, steps > 0) {
                (false, true) => 4,
                (false, false) => 5,
                (true, true) => 7,
                (true, false) => 6,
            };
            if let Some(data) = ptr.as_ref() {
                // Wheel messages carry screen coordinates.
                let (sx, sy) = lparam_point(lp);
                let mut pt = POINT { x: sx, y: sy };
                let _ = ScreenToClient(hwnd, &mut pt);
                on_button(hwnd, data, button, false, pt.x, pt.y, steps.unsigned_abs());
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            if !ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
