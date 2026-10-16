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

fn open_pipe(name: &str) -> std::fs::File {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(name)
        {
            Ok(f) => return f,
            Err(e) if Instant::now() >= end => panic!("cannot open {name}: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn read_frame(f: &mut std::fs::File) -> (u32, serde_json::Value) {
    use std::io::Read;
    let mut h = [0u8; i3win_core::ipc::HEADER_LEN];
    f.read_exact(&mut h).unwrap();
    let (len, ty) = i3win_core::ipc::decode_header(&h).unwrap();
    let mut body = vec![0u8; len as usize];
    f.read_exact(&mut body).unwrap();
    (ty, serde_json::from_slice(&body).unwrap())
}

fn request(f: &mut std::fs::File, ty: u32, payload: &str) -> serde_json::Value {
    use std::io::Write;
    f.write_all(&i3win_core::ipc::encode(ty, payload)).unwrap();
    let (rty, v) = read_frame(f);
    assert_eq!(rty, ty);
    v
}

fn ipc_run(children: &mut Vec<Child>) {
    use i3win_core::ipc::{GET_VERSION, GET_WORKSPACES, RUN_COMMAND, SUBSCRIBE};
    let pipe = format!(r"\.\pipe\i3win-test-{}", std::process::id());
    let cfg = std::env::temp_dir().join("i3win-smoke-ipc-config");
    std::fs::write(&cfg, "default_border none\n").unwrap();
    children.push(
        Command::new(env!("CARGO_BIN_EXE_i3win"))
            .arg("--config")
            .arg(&cfg)
            .arg("--ipc-pipe")
            .arg(&pipe)
            .spawn()
            .unwrap(),
    );
    let mut a = open_pipe(&pipe);
    assert_eq!(request(&mut a, GET_VERSION, "")["major"], 4);
    assert_eq!(
        request(&mut a, RUN_COMMAND, "workspace 5"),
        serde_json::json!([{"success": true}])
    );
    let ws = request(&mut a, GET_WORKSPACES, "");
    let five = ws
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "5")
        .expect("workspace 5 listed");
    assert_eq!(five["focused"], true);

    let mut b = open_pipe(&pipe);
    assert_eq!(
        request(&mut b, SUBSCRIBE, r#"["workspace"]"#),
        serde_json::json!({"success": true})
    );
    request(&mut a, RUN_COMMAND, "workspace 6");
    let (ty, ev) = read_frame(&mut b);
    assert_eq!(ty, 0x8000_0000);
    assert_eq!(ev["change"], "focus");
}

#[test]
#[ignore]
fn ipc_roundtrip_and_events() {
    let mut children = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ipc_run(&mut children)));
    for c in children.iter_mut() {
        let _ = c.kill();
    }
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
}
