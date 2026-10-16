//! End-to-end smoke test: two Notepads tile side by side.
//! Rearranges the real desktop, so it is ignored by default:
//! `cargo test -p i3win --test smoke -- --ignored --test-threads=1`

use std::process::{Child, Command};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
};

struct Search {
    pid: u32,
    found: Option<isize>,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lp: LPARAM) -> BOOL {
    let s = &mut *(lp.0 as *mut Search);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == s.pid && IsWindowVisible(hwnd).as_bool() {
        s.found = Some(hwnd.0 as isize);
        return BOOL(0);
    }
    TRUE
}

fn visible_window(pid: u32) -> Option<isize> {
    let mut s = Search { pid, found: None };
    // SAFETY: `s` outlives the synchronous EnumWindows call.
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut s as *mut Search as isize));
    }
    s.found
}

fn frame(h: isize) -> RECT {
    let mut r = RECT::default();
    // SAFETY: `r` is a valid RECT of the size we pass.
    unsafe {
        let _ = DwmGetWindowAttribute(
            HWND(h as *mut _),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
    }
    r
}

fn work_area() -> RECT {
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `mi` has cbSize set.
    unsafe {
        let m = MonitorFromPoint(Default::default(), MONITOR_DEFAULTTOPRIMARY);
        let _ = GetMonitorInfoW(m, &mut mi);
    }
    mi.rcWork
}

fn wait_window(pid: u32) -> Option<isize> {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if let Some(h) = visible_window(pid) {
            return Some(h);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn run(children: &mut Vec<Child>) {
    let cfg = std::env::temp_dir().join("i3win-smoke-config");
    std::fs::write(&cfg, "default_border none\nbindsym Mod1+Shift+F12 exit\n").unwrap();
    children.push(
        Command::new(env!("CARGO_BIN_EXE_i3win"))
            .arg("--config")
            .arg(&cfg)
            .spawn()
            .unwrap(),
    );
    std::thread::sleep(Duration::from_secs(1));

    let mut pids = Vec::new();
    for _ in 0..2 {
        let c = Command::new("notepad.exe").spawn().unwrap();
        pids.push(c.id());
        children.push(c);
    }
    let hs: Vec<isize> = pids
        .iter()
        .map(|p| wait_window(*p).expect("notepad window did not appear"))
        .collect();
    std::thread::sleep(Duration::from_secs(1));

    let mut fr: Vec<RECT> = hs.iter().map(|h| frame(*h)).collect();
    fr.sort_by_key(|r| r.left);
    let (l, r) = (fr[0], fr[1]);
    let wa = work_area();
    let half = (wa.right - wa.left) / 2;

    assert!(l.right <= r.left + 2, "frames overlap: {l:?} {r:?}");
    assert!((l.right - r.left).abs() <= 2, "gap/overlap: {l:?} {r:?}");
    for f in [l, r] {
        assert!(
            ((f.right - f.left) - half).abs() <= 10,
            "width {f:?} vs half {half}"
        );
        assert_eq!(f.top, wa.top, "top {f:?} vs work area {wa:?}");
    }
}

#[test]
#[ignore]
fn two_notepads_tile_side_by_side() {
    let mut children = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut children)));
    for c in children.iter_mut() {
        let _ = c.kill();
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
