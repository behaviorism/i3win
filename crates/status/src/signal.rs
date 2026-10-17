//! `--signal N` / `--refresh-all` delivery: a message-only window per
//! running i3win-status, found by window class from other processes.

use crate::Event;
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, GetMessageW, PostMessageW,
    RegisterClassW, TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP,
    WNDCLASSW,
};

pub const SIGNAL_CLASS: &str = "i3win_status_sig";
const WM_SIGNAL: u32 = WM_APP + 1;
const WM_REFRESH_ALL: u32 = WM_APP + 2;

static SINK: OnceLock<Sender<Event>> = OnceLock::new();

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SIGNAL => {
            if let Some(tx) = SINK.get() {
                let _ = tx.send(Event::Signal(wp.0 as u32));
            }
            LRESULT(0)
        }
        WM_REFRESH_ALL => {
            if let Some(tx) = SINK.get() {
                let _ = tx.send(Event::RefreshAll);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Spawns the listener thread: it registers the class, creates the window and
/// pumps messages on the same thread.
pub fn listen(tx: Sender<Event>) {
    let _ = SINK.set(tx);
    std::thread::spawn(|| unsafe {
        let class_name = HSTRING::from(SIGNAL_CLASS);
        let class = PCWSTR(class_name.as_ptr());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            None,
            None,
            None,
        ) else {
            eprintln!("i3win-status: cannot create signal window");
            return;
        };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, hwnd, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

/// Posts `msg` to every signal window; returns how many were found.
fn post_all(msg: u32, wparam: usize) -> usize {
    let mut count = 0;
    let class = HSTRING::from(SIGNAL_CLASS);
    let mut prev = HWND::default();
    loop {
        let found = unsafe { FindWindowExW(HWND_MESSAGE, prev, &class, PCWSTR::null()) };
        match found {
            Ok(h) if !h.is_invalid() => {
                if unsafe { PostMessageW(h, msg, WPARAM(wparam), LPARAM(0)) }.is_ok() {
                    count += 1;
                }
                prev = h;
            }
            _ => return count,
        }
    }
}

pub fn send_signal(n: u32) -> usize {
    post_all(WM_SIGNAL, n as usize)
}

pub fn send_refresh_all() -> usize {
    post_all(WM_REFRESH_ALL, 0)
}
