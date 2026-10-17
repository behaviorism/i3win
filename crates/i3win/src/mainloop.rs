use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;

use i3win_core::tree::NodeId;

use crate::app::{App, APP};
use crate::win::{events, hidden, keyboard, window};
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
    RunCommand(String),
    /// A click on a status block of bar `bar`.
    StatusClick {
        bar: String,
        click: i3win_core::i3bar::Click,
    },
    /// Blocks from the status process of bar (id) and generation.
    StatusUpdate(String, u64, Vec<i3win_core::i3bar::Block>),
    /// The status process of bar (id) and generation ended.
    StatusExited(String, u64),
    /// An i3 IPC request from a connection thread; the answer goes back on `reply`.
    Ipc {
        msg_type: u32,
        payload: String,
        reply: std::sync::mpsc::Sender<String>,
    },
    Shutdown,
    /// Debug-only crash-safety trigger; the main thread panics on it.
    #[cfg(debug_assertions)]
    DebugPanic,
}

/// Serializes tests that use the process-global queue.
#[cfg(test)]
pub(crate) static QUEUE_TEST_LOCK: Mutex<()> = Mutex::new(());

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static QUEUE: Mutex<VecDeque<MainMsg>> = Mutex::new(VecDeque::new());
/// True while a WM_APP_MAIN wake-up is posted and not yet being handled.
static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

/// Returns true when the caller must post a wake-up (none is pending yet).
fn claim_wake(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::AcqRel)
}

/// Raw handle of the main window, or 0 before it exists.
pub fn main_hwnd() -> isize {
    MAIN_HWND.load(Ordering::SeqCst)
}

/// Queue a message for the main thread; callable from any thread.
/// If no main window exists yet, the message stays queued.
pub fn post(msg: MainMsg) {
    if let Ok(mut q) = QUEUE.lock() {
        q.push_back(msg);
    }
    let h = MAIN_HWND.load(Ordering::SeqCst);
    if h != 0 && claim_wake(&WAKE_PENDING) {
        unsafe {
            if PostMessageW(HWND(h as *mut _), WM_APP_MAIN, WPARAM(0), LPARAM(0)).is_err() {
                WAKE_PENDING.store(false, Ordering::Release);
            }
        }
    }
}

pub fn drain_main_msgs() -> Vec<MainMsg> {
    match QUEUE.lock() {
        Ok(mut q) => q.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

thread_local! {
    /// Set after a handler panic; only Shutdown is processed from then on.
    static POISONED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

const SPI_SETWORKAREA: usize = 0x2F;

/// Runs `f` against the App if it is not already borrowed; returns false when busy.
/// A panic inside `f` restores all hidden windows and shuts down.
fn with_app(f: impl FnOnce(&mut App)) -> bool {
    if POISONED.with(|p| p.get()) {
        return true;
    }
    APP.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return false;
        };
        if let Some(app) = guard.as_mut() {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(|| f(app))) {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic".into());
                log::error!("panic in message handler: {msg}");
                POISONED.with(|p| p.set(true));
                hidden::restore_all_hidden();
                post(MainMsg::Shutdown);
            }
        }
        true
    })
}

fn repost(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) {
    // SAFETY: plain Win32 call on our own window.
    unsafe {
        let _ = PostMessageW(hwnd, msg, wp, lp);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_MAIN => {
            // Clear before draining: a post after the drain starts a new wake-up.
            WAKE_PENDING.store(false, Ordering::Release);
            let mut rest = Vec::new();
            for m in drain_main_msgs() {
                match m {
                    MainMsg::Shutdown => PostQuitMessage(0),
                    other => rest.push(other),
                }
            }
            if !rest.is_empty() {
                let mut slot = Some(rest);
                let done = with_app(|app| {
                    let mut need_flush = false;
                    for m in slot.take().into_iter().flatten() {
                        need_flush |= app.on_main_msg(m);
                    }
                    app.finish_main_msgs(need_flush);
                });
                if !done {
                    // App busy: put the messages back; `post` re-posts the wake-up.
                    for m in slot.take().into_iter().flatten() {
                        post(m);
                    }
                }
            }
            LRESULT(0)
        }
        WM_APP_EVENTS => {
            if !with_app(|app| {
                for ev in events::drain() {
                    app.on_event(ev);
                }
                app.flush();
            }) {
                repost(hwnd, msg, wp, lp);
            }
            LRESULT(0)
        }
        WM_APP_KEY => {
            if !with_app(|app| {
                for (combo, mask_menu, release) in keyboard::drain() {
                    if mask_menu {
                        window::send_dummy_key();
                    }
                    if let Some(combo) = combo {
                        app.on_binding(combo, release);
                    }
                }
                app.flush();
            }) {
                repost(hwnd, msg, wp, lp);
            }
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            if !with_app(|app| app.refresh_outputs()) {
                repost(hwnd, msg, wp, lp);
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE if wp.0 == SPI_SETWORKAREA => {
            if !with_app(|app| app.refresh_outputs()) {
                repost(hwnd, msg, wp, lp);
            }
            LRESULT(0)
        }
        WM_ENDSESSION if wp.0 != 0 => {
            hidden::restore_all_hidden();
            hidden::clear_store_file(&crate::paths::hidden_path());
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
        WAKE_PENDING.store(true, Ordering::Release);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_is_claimed_once_until_cleared() {
        let f = AtomicBool::new(false);
        assert!(claim_wake(&f));
        assert!(!claim_wake(&f));
        f.store(false, Ordering::Release);
        assert!(claim_wake(&f));
    }

    #[test]
    fn thousand_posts_without_window_are_all_queued() {
        let _g = QUEUE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(main_hwnd(), 0);
        let _ = drain_main_msgs();
        for i in 0..1000 {
            post(MainMsg::RunCommand(i.to_string()));
        }
        assert_eq!(drain_main_msgs().len(), 1000);
        assert!(!WAKE_PENDING.load(Ordering::Acquire));
    }
}
