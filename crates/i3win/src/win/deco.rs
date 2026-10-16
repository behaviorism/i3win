//! Decoration windows: title bars, borders and tab/stack bars drawn with GDI.
//!
//! The window procedure never touches `APP`: it paints from the `DecoData`
//! stored in `GWLP_USERDATA` and only posts `MainMsg::FocusNode` on clicks.
use std::collections::{HashMap, HashSet};

use i3win_core::arrange::{Arrangement, Deco, Tab};
use i3win_core::config::{ClientColors, Config, Font};
use i3win_core::geom::Rect;
use i3win_core::tree::NodeId;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::mainloop::{self, MainMsg};
use crate::win::gdi;

/// Everything a deco window needs to paint itself.
#[derive(Clone, Debug, PartialEq)]
pub struct DecoData {
    pub deco: Deco,
    pub colors: ClientColors,
    pub font: Font,
    pub scale: f32,
}

pub struct Decos {
    windows: HashMap<NodeId, (HWND, DecoData)>,
}

pub fn deco_key(d: &Deco) -> NodeId {
    match d {
        Deco::Frame { key, .. } | Deco::TabBar { key, .. } => *key,
    }
}

fn deco_rect(d: &Deco) -> Rect {
    match d {
        Deco::Frame { rect, .. } | Deco::TabBar { rect, .. } => *rect,
    }
}

/// Maps a screen point to the tab under it. Tab rects are in screen
/// coordinates, so `origin` (the deco window's screen top-left) is not needed
/// for the lookup; it is kept for callers working in window-relative terms.
pub fn tab_at(tabs: &[Tab], _origin: (i32, i32), x: i32, y: i32) -> Option<NodeId> {
    tabs.iter()
        .find(|t| x >= t.rect.x && x < t.rect.right() && y >= t.rect.y && y < t.rect.bottom())
        .map(|t| t.node)
}

fn register_class() {
    // SAFETY: plain class registration; "already exists" is fine.
    unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else {
            return;
        };
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(deco_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("i3win_deco"),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            log::error!("registering i3win_deco failed");
        }
    }
}

fn create_window(data: &DecoData) -> Option<HWND> {
    let r = deco_rect(&data.deco);
    // SAFETY: creates a window of our own class; ownership of the boxed data
    // passes to GWLP_USERDATA and is freed in WM_NCDESTROY.
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("i3win_deco"),
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
        .map_err(|e| log::error!("creating deco window failed: {e}"))
        .ok()?;
        let ptr = Box::into_raw(Box::new(data.clone()));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        Some(hwnd)
    }
}

fn set_data(hwnd: HWND, data: &DecoData) {
    // SAFETY: the pointer is the live Box stored by `create_window`; we are on
    // the UI thread and no paint is in progress while this runs.
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DecoData;
        if let Some(slot) = p.as_mut() {
            *slot = data.clone();
        }
        let _ = InvalidateRect(hwnd, None, false);
    }
}

impl Default for Decos {
    fn default() -> Self {
        Self::new()
    }
}

impl Decos {
    pub fn new() -> Decos {
        register_class();
        Decos {
            windows: HashMap::new(),
        }
    }

    /// Brings the deco windows in line with `arr`.
    pub fn reconcile(&mut self, arr: &Arrangement, cfg: &Config, scale_of: impl Fn(Rect) -> f32) {
        let live: HashSet<NodeId> = arr.decos.iter().map(deco_key).collect();
        let stale: Vec<NodeId> = self
            .windows
            .keys()
            .filter(|k| !live.contains(k))
            .copied()
            .collect();
        for k in stale {
            if let Some((hwnd, _)) = self.windows.remove(&k) {
                // SAFETY: destroying our own window.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
        for d in &arr.decos {
            let key = deco_key(d);
            let r = deco_rect(d);
            let data = DecoData {
                deco: d.clone(),
                colors: cfg.colors,
                font: cfg.font.clone(),
                scale: scale_of(r),
            };
            let (hwnd, created, changed, old_rect) = match self.windows.get(&key) {
                Some((h, old)) => (*h, false, *old != data, deco_rect(&old.deco)),
                None => match create_window(&data) {
                    Some(h) => (h, true, false, r),
                    None => continue,
                },
            };
            if changed {
                set_data(hwnd, &data);
            }
            let (w, h) = (r.w.max(1), r.h.max(1));
            // SAFETY: plain Win32 calls on our own window.
            unsafe {
                match d {
                    Deco::Frame { above, .. } => {
                        let _ = SetWindowPos(
                            hwnd,
                            HWND(above.0 as *mut _),
                            r.x,
                            r.y,
                            w,
                            h,
                            SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        );
                    }
                    Deco::TabBar { .. } => {
                        if created {
                            let _ = SetWindowPos(
                                hwnd,
                                HWND_TOP,
                                r.x,
                                r.y,
                                w,
                                h,
                                SWP_NOACTIVATE | SWP_SHOWWINDOW,
                            );
                        } else if old_rect != r {
                            let _ = SetWindowPos(
                                hwnd,
                                HWND::default(),
                                r.x,
                                r.y,
                                w,
                                h,
                                SWP_NOACTIVATE | SWP_NOZORDER,
                            );
                        }
                    }
                }
            }
            self.windows.insert(key, (hwnd, data));
        }
    }

    /// Destroys every deco window and frees cached fonts.
    pub fn destroy_all(&mut self) {
        for (_, (hwnd, _)) in self.windows.drain() {
            // SAFETY: destroying our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
        gdi::free_fonts();
    }
}

fn paint(hwnd: HWND, data: &DecoData) {
    let font = gdi::font(&data.font.family, data.font.size_pt, data.scale);
    let inset = (4.0 * data.scale).round() as i32;
    gdi::paint_buffered(hwnd, |hdc, client| match &data.deco {
        Deco::Frame {
            title_rect,
            title,
            state,
            ..
        } => {
            let cc = gdi::colors_for(*state, &data.colors);
            gdi::fill(hdc, client, cc.child_border);
            if let Some(t) = title_rect {
                let area = Rect::new(0, 0, client.w, t.h);
                gdi::fill(hdc, area, cc.background);
                gdi::outline(hdc, area, cc.border);
                let text = area.inset(inset, 0, inset, 0);
                gdi::draw_text(hdc, text, title, cc.text, font);
            }
        }
        Deco::TabBar { rect, tabs, .. } => {
            for t in tabs {
                let cc = gdi::colors_for(t.state, &data.colors);
                let r = Rect::new(t.rect.x - rect.x, t.rect.y - rect.y, t.rect.w, t.rect.h);
                gdi::fill(hdc, r, cc.background);
                gdi::outline(hdc, r, cc.border);
                gdi::draw_text(hdc, r.inset(inset, 0, inset, 0), &t.title, cc.text, font);
            }
        }
    });
}

unsafe extern "system" fn deco_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DecoData;
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
        WM_LBUTTONDOWN => {
            if let Some(data) = ptr.as_ref() {
                match &data.deco {
                    Deco::Frame { key, .. } => mainloop::post(MainMsg::FocusNode(*key)),
                    Deco::TabBar { tabs, rect, .. } => {
                        let mut pt = POINT::default();
                        if GetCursorPos(&mut pt).is_ok() {
                            if let Some(n) = tab_at(tabs, (rect.x, rect.y), pt.x, pt.y) {
                                mainloop::post(MainMsg::FocusNode(n));
                            }
                        }
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use i3win_core::arrange::{Deco, DecoState, Tab};
    use i3win_core::geom::Rect;
    use i3win_core::tree::NodeId;
    #[test]
    fn tab_hit_testing() {
        let tabs = vec![
            Tab {
                title: "a".into(),
                state: DecoState::Unfocused,
                rect: Rect::new(100, 0, 50, 20),
                node: NodeId(1),
            },
            Tab {
                title: "b".into(),
                state: DecoState::Focused,
                rect: Rect::new(150, 0, 50, 20),
                node: NodeId(2),
            },
        ];
        assert_eq!(tab_at(&tabs, (100, 0), 160, 10), Some(NodeId(2)));
        assert_eq!(tab_at(&tabs, (100, 0), 99, 10), None);
    }
    #[test]
    fn key_of_deco() {
        let d = Deco::TabBar {
            key: NodeId(9),
            rect: Rect::default(),
            vertical: false,
            tabs: vec![],
        };
        assert_eq!(deco_key(&d), NodeId(9));
    }
}
