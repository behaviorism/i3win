use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::mem::size_of;

use i3win_core::geom::Rect;
use i3win_core::tree::{WindowId, WindowInfo};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, POINT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindow,
    GetWindowLongW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, PostMessageW, SetCursorPos, SetForegroundWindow, SetWindowPos,
    ShowWindowAsync, WindowFromPoint, GA_ROOT, GWL_EXSTYLE, GWL_STYLE, GW_OWNER, HWND_TOP,
    SET_WINDOW_POS_FLAGS, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER,
    SW_HIDE, SW_RESTORE, SW_SHOWNA, WM_CLOSE,
};

const WS_CHILD: u32 = 0x4000_0000;
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_EX_DLGMODALFRAME: u32 = 0x1;
const WS_EX_TOOLWINDOW: u32 = 0x80;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

const IGNORED_CLASSES: [&str; 6] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "TaskManagerWindow",
];

/// Raw facts about a window, gathered from Win32, that drive classification.
pub struct WindowProps {
    pub visible: bool,
    pub minimized: bool,
    pub style: u32,
    pub ex_style: u32,
    pub has_owner: bool,
    pub cloaked: bool,
    pub own_process: bool,
    pub title: String,
    pub class_name: String,
    pub is_child: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Manage {
    No,
    Tile,
    Float,
}

pub fn classify(p: &WindowProps) -> Manage {
    if !p.visible
        || p.minimized
        || p.is_child
        || p.has_owner
        || p.cloaked
        || p.own_process
        || p.title.is_empty()
    {
        return Manage::No;
    }
    if p.ex_style & WS_EX_TOOLWINDOW != 0 || p.ex_style & WS_EX_NOACTIVATE != 0 {
        return Manage::No;
    }
    if p.style & WS_CAPTION != WS_CAPTION {
        return Manage::No;
    }
    if IGNORED_CLASSES.contains(&p.class_name.as_str()) {
        return Manage::No;
    }
    if p.ex_style & WS_EX_DLGMODALFRAME != 0 || p.style & WS_THICKFRAME == 0 {
        return Manage::Float;
    }
    Manage::Tile
}

/// Rect to hand to `SetWindowPos` so that the visible frame equals `target`.
pub fn compensate(target: Rect, window_rect: Rect, frame_rect: Rect) -> Rect {
    let left = frame_rect.x.saturating_sub(window_rect.x);
    let top = frame_rect.y.saturating_sub(window_rect.y);
    let right = window_rect.right().saturating_sub(frame_rect.right());
    let bottom = window_rect.bottom().saturating_sub(frame_rect.bottom());
    Rect::new(
        target.x.saturating_sub(left),
        target.y.saturating_sub(top),
        target.w.saturating_add(left).saturating_add(right),
        target.h.saturating_add(top).saturating_add(bottom),
    )
}

/// File name without directory or extension (case kept).
pub fn exe_stem(path: &str) -> String {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].to_string(),
        _ => name.to_string(),
    }
}

fn hwnd(id: WindowId) -> HWND {
    HWND(id.0 as *mut c_void)
}

fn rect_from(r: &RECT) -> Rect {
    Rect::new(
        r.left,
        r.top,
        r.right.saturating_sub(r.left),
        r.bottom.saturating_sub(r.top),
    )
}

fn window_text(h: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: buf is a valid writable buffer for the duration of the call.
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.clamp(0, 512) as usize])
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: buf is a valid writable buffer for the duration of the call.
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.clamp(0, 256) as usize])
}

pub fn window_pid(id: WindowId) -> u32 {
    process_id(hwnd(id))
}

fn process_id(h: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: pid is a valid out pointer.
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

fn exe_name(pid: u32) -> String {
    if pid == 0 {
        return String::new();
    }
    // SAFETY: plain Win32 calls; the handle is closed before returning.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(handle);
        if ok {
            exe_stem(&String::from_utf16_lossy(
                &buf[..(len as usize).min(buf.len())],
            ))
        } else {
            String::new()
        }
    }
}

pub fn props(id: WindowId) -> Option<WindowProps> {
    let h = hwnd(id);
    // SAFETY: plain Win32 queries on a possibly stale handle; failures are handled.
    unsafe {
        if !IsWindow(h).as_bool() {
            return None;
        }
        let style = GetWindowLongW(h, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        let has_owner = GetWindow(h, GW_OWNER).is_ok_and(|o| !o.0.is_null());
        let mut cloak = 0u32;
        let cloaked = DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            &mut cloak as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloak != 0;
        Some(WindowProps {
            visible: IsWindowVisible(h).as_bool(),
            minimized: IsIconic(h).as_bool(),
            style,
            ex_style,
            has_owner,
            cloaked,
            own_process: process_id(h) == GetCurrentProcessId(),
            title: window_text(h),
            class_name: class_name(h),
            is_child: style & WS_CHILD != 0,
        })
    }
}

pub fn info(id: WindowId, wants_float: bool) -> Option<WindowInfo> {
    let h = hwnd(id);
    let rect = frame_rect(id)?;
    Some(WindowInfo {
        id,
        title: window_text(h),
        class: exe_name(process_id(h)),
        instance: class_name(h),
        rect,
        wants_float,
    })
}

fn raw_window_rect(id: WindowId) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: r is a valid out pointer.
    unsafe { GetWindowRect(hwnd(id), &mut r) }.ok()?;
    Some(rect_from(&r))
}

pub fn frame_rect(id: WindowId) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: r is a valid out buffer of the stated size.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd(id),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    }
    .is_ok();
    if ok {
        Some(rect_from(&r))
    } else {
        raw_window_rect(id)
    }
}

thread_local! {
    static WARNED: RefCell<HashSet<WindowId>> = RefCell::new(HashSet::new());
}

pub fn place(id: WindowId, target: Rect, raise: bool) {
    let h = hwnd(id);
    // SAFETY: plain Win32 call on a possibly stale handle.
    unsafe {
        if IsZoomed(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }
    }
    let r = match (raw_window_rect(id), frame_rect(id)) {
        (Some(win), Some(frame)) => compensate(target, win, frame),
        _ => target,
    };
    let mut flags: SET_WINDOW_POS_FLAGS = SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_NOOWNERZORDER;
    if !raise {
        flags |= SWP_NOZORDER;
    }
    let after = if raise { HWND_TOP } else { HWND::default() };
    // SAFETY: plain Win32 call.
    let res = unsafe { SetWindowPos(h, after, r.x, r.y, r.w, r.h, flags) };
    match res {
        Err(e) => {
            if WARNED.with(|w| w.borrow_mut().insert(id)) {
                log::warn!("SetWindowPos failed for window {:#x}: {e}", id.0);
            }
        }
        Ok(()) => WARNED.with(|w| {
            w.borrow_mut().remove(&id);
        }),
    }
}

thread_local! {
    static HIDE_WARNED: RefCell<HashSet<WindowId>> = RefCell::new(HashSet::new());
}

/// Hides the window; returns whether the hide request was accepted.
pub fn hide(id: WindowId) -> bool {
    // SAFETY: plain Win32 call.
    let ok = unsafe { ShowWindowAsync(hwnd(id), SW_HIDE) }.as_bool();
    if ok {
        HIDE_WARNED.with(|w| {
            w.borrow_mut().remove(&id);
        });
    } else if HIDE_WARNED.with(|w| w.borrow_mut().insert(id)) {
        log::warn!("ShowWindowAsync(SW_HIDE) failed for window {:#x}", id.0);
    }
    ok
}

pub fn is_visible(id: WindowId) -> bool {
    // SAFETY: plain Win32 call.
    unsafe { IsWindowVisible(hwnd(id)).as_bool() }
}

pub fn show(id: WindowId) {
    // SAFETY: plain Win32 call.
    unsafe {
        let _ = ShowWindowAsync(hwnd(id), SW_SHOWNA);
    }
}

pub fn close(id: WindowId) {
    // SAFETY: plain Win32 call; PostMessage never blocks on the target.
    unsafe {
        let _ = PostMessageW(hwnd(id), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn focus(id: WindowId) {
    let h = hwnd(id);
    // SAFETY: plain Win32 call.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }
    }
    send_dummy_key();
    // SAFETY: plain Win32 call.
    let ok = unsafe { SetForegroundWindow(h) }.as_bool();
    log::debug!("SetForegroundWindow({:#x}) -> {ok}", id.0);
}

/// Injects an unassigned key press so Windows lets us take the foreground.
pub fn send_dummy_key() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0xE8),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: crate::win::keyboard::OWN_INPUT_TAG,
            },
        },
    };
    let inputs = [key(Default::default()), key(KEYEVENTF_KEYUP)];
    // SAFETY: inputs is a valid slice of INPUT structs.
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    log::debug!("SendInput injected {sent} of {} events", inputs.len());
}

pub fn is_window(id: WindowId) -> bool {
    // SAFETY: plain Win32 call.
    unsafe { IsWindow(hwnd(id)).as_bool() }
}

unsafe extern "system" fn enum_proc(h: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut Vec<WindowId> passed by enumerate_top_level.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<WindowId>) };
    out.push(WindowId(h.0 as isize));
    TRUE
}

/// All top-level windows, top of the z-order first.
pub fn enumerate_top_level() -> Vec<WindowId> {
    let mut out: Vec<WindowId> = Vec::new();
    // SAFETY: the pointer outlives the synchronous EnumWindows call.
    let _ = unsafe {
        EnumWindows(
            Some(enum_proc),
            LPARAM(&mut out as *mut Vec<WindowId> as isize),
        )
    };
    out
}

/// Moves the pointer to screen point (x, y).
pub fn set_cursor_pos(x: i32, y: i32) {
    // SAFETY: plain Win32 call.
    let _ = unsafe { SetCursorPos(x, y) };
}

/// The pointer position in screen coordinates.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: p is a valid out pointer for the synchronous call.
    unsafe { GetCursorPos(&mut p).ok().map(|()| (p.x, p.y)) }
}

/// The top-level window under screen point (x, y), if any.
pub fn window_at_point(x: i32, y: i32) -> Option<WindowId> {
    // SAFETY: plain Win32 calls.
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.0.is_null() {
            return None;
        }
        let root = GetAncestor(h, GA_ROOT);
        let root = if root.0.is_null() { h } else { root };
        Some(WindowId(root.0 as isize))
    }
}

pub fn foreground() -> Option<WindowId> {
    // SAFETY: plain Win32 call.
    let h = unsafe { GetForegroundWindow() };
    if h.0.is_null() {
        None
    } else {
        Some(WindowId(h.0 as isize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn base() -> WindowProps {
        WindowProps {
            visible: true,
            minimized: false,
            style: 0x00C0_0000 | 0x0004_0000,
            ex_style: 0,
            has_owner: false,
            cloaked: false,
            own_process: false,
            title: "t".into(),
            class_name: "Notepad".into(),
            is_child: false,
        }
    }
    #[test]
    fn normal_window_tiles() {
        assert_eq!(classify(&base()), Manage::Tile);
    }
    #[test]
    fn rejects() {
        let fs: [fn(&mut WindowProps); 9] = [
            |p| p.visible = false,
            |p| p.minimized = true,
            |p| p.has_owner = true,
            |p| p.cloaked = true,
            |p| p.own_process = true,
            |p| p.title.clear(),
            |p| p.ex_style = 0x80,
            |p| p.style = 0x0004_0000,
            |p| p.class_name = "Shell_TrayWnd".into(),
        ];
        for f in fs {
            let mut p = base();
            f(&mut p);
            assert_eq!(classify(&p), Manage::No);
        }
    }
    #[test]
    fn dialogs_and_fixed_size_float() {
        let mut p = base();
        p.ex_style = 0x1;
        assert_eq!(classify(&p), Manage::Float);
        let mut p = base();
        p.style = 0x00C0_0000;
        assert_eq!(classify(&p), Manage::Float);
    }
    #[test]
    fn compensate_for_invisible_borders() {
        // window rect is 7px wider on left/right/bottom than its visible frame
        let win = Rect::new(93, 100, 414, 307);
        let frame = Rect::new(100, 100, 400, 300);
        assert_eq!(
            compensate(Rect::new(0, 0, 500, 500), win, frame),
            Rect::new(-7, 0, 514, 507)
        );
    }
    #[test]
    fn exe_stem_strips_dir_and_ext() {
        assert_eq!(
            exe_stem(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            "firefox"
        );
        assert_eq!(exe_stem("Code.exe"), "Code");
    }
    #[test]
    fn compensate_saturates() {
        let win = Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        let frame = Rect::new(i32::MAX, i32::MAX, 1, 1);
        let _ = compensate(
            Rect::new(i32::MAX, i32::MAX, i32::MAX, i32::MAX),
            win,
            frame,
        );
    }
    #[test]
    fn exe_stem_slash_and_no_ext() {
        assert_eq!(exe_stem("C:/x/y/app.exe"), "app");
        assert_eq!(exe_stem(r"C:\x\noext"), "noext");
    }
}
