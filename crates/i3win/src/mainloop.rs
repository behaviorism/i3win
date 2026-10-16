use std::collections::VecDeque;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

use i3win_core::tree::NodeId;
use windows::core::{w, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_EVENTS: u32 = WM_APP + 1;
pub const WM_APP_KEY: u32 = WM_APP + 2;
pub const WM_APP_MAIN: u32 = WM_APP + 3;

#[derive(Debug)]
pub enum MainMsg {
    FocusNode(NodeId),
    Shutdown,
}

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static QUEUE: Mutex<VecDeque<MainMsg>> = Mutex::new(VecDeque::new());

/// Queue a message for the main thread; callable from any thread.
/// If no main window exists yet, the message stays queued.
pub fn post(msg: MainMsg) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push_back(msg);
    }
    let h = MAIN_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h as *mut _), WM_APP_MAIN, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn drain_main_msgs() -> Vec<MainMsg> {
    match QUEUE.lock() {
        Ok(mut q) => q.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_MAIN => {
            for m in drain_main_msgs() {
                match m {
                    MainMsg::Shutdown => PostQuitMessage(0),
                    MainMsg::FocusNode(_) => {}
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Registers class `i3win_main` and creates the invisible top-level main window.
pub fn create_main_window() -> Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = w!("i3win_main");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_win32());
        }
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!("i3win"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        )?;
        MAIN_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        // Deliver anything queued before the window existed.
        let _ = PostMessageW(hwnd, WM_APP_MAIN, WPARAM(0), LPARAM(0));
        Ok(hwnd)
    }
}

/// Message loop; returns the `WM_QUIT` exit code.
pub fn run() -> i32 {
    unsafe {
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 {
                return msg.wParam.0 as i32;
            }
            if r.0 < 0 {
                return 1;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
