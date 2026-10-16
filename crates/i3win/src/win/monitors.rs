//! Monitor enumeration.
use std::mem::size_of;

use i3win_core::geom::Rect;
use i3win_core::wm::OutputInfo;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

/// Strips the leading device-namespace prefix from a GDI device name (DISPLAY1 from the full path).
pub fn output_name(device: &str) -> String {
    device.strip_prefix(r"\.\").unwrap_or(device).to_string()
}

/// Orders outputs left-to-right, then top-to-bottom.
pub fn sort_outputs(v: &mut [OutputInfo]) {
    v.sort_by_key(|o| (o.rect.x, o.rect.y));
}

unsafe extern "system" fn enum_proc(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the address of the Vec owned by `enumerate`, which
    // outlives the synchronous EnumDisplayMonitors call.
    let out = &mut *(data.0 as *mut Vec<OutputInfo>);
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if !GetMonitorInfoW(
        monitor,
        &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
    )
    .as_bool()
    {
        return TRUE;
    }
    let len = info
        .szDevice
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(info.szDevice.len());
    let name = output_name(&String::from_utf16_lossy(&info.szDevice[..len]));
    let w = info.monitorInfo.rcWork;
    let (mut dx, mut dy) = (0u32, 0u32);
    let scale = match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) {
        Ok(()) if dx > 0 => dx as f32 / 96.0,
        _ => 1.0,
    };
    out.push(OutputInfo {
        name,
        rect: Rect::new(w.left, w.top, w.right - w.left, w.bottom - w.top),
        scale,
    });
    TRUE
}

/// Lists all monitors (work area rects), sorted left-to-right.
pub fn enumerate() -> Vec<OutputInfo> {
    let mut out: Vec<OutputInfo> = Vec::new();
    // SAFETY: the callback only touches the Vec passed via LPARAM and never panics.
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(enum_proc),
            LPARAM(&mut out as *mut Vec<OutputInfo> as isize),
        );
    }
    sort_outputs(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use i3win_core::geom::Rect;

    #[test]
    fn names() {
        assert_eq!(output_name(r"\.\DISPLAY2"), "DISPLAY2");
        assert_eq!(output_name("X"), "X");
    }
    #[test]
    fn sorted_left_to_right() {
        let mut v = vec![
            OutputInfo {
                name: "B".into(),
                rect: Rect::new(1920, 0, 1920, 1040),
                scale: 1.0,
            },
            OutputInfo {
                name: "A".into(),
                rect: Rect::new(0, 0, 1920, 1040),
                scale: 1.0,
            },
        ];
        sort_outputs(&mut v);
        assert_eq!(v[0].name, "A");
    }
    #[test]
    fn enumerate_finds_at_least_one() {
        assert!(!enumerate().is_empty());
    }
}
