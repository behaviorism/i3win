//! Bar windows: one topmost, never-activating GDI strip per output.
//!
//! The window procedure never touches `APP`: it paints from the `BarData`
//! stored in `GWLP_USERDATA` and only posts `MainMsg`s on clicks and wheel turns.
use std::collections::{HashMap, HashSet};

use i3win_core::arrange::{bar_height, Arrangement};
use i3win_core::bar_model::workspace_buttons;
use i3win_core::config::{BarPosition, Color, Font};
use i3win_core::geom::Rect;
use i3win_core::i3bar::Block;
use i3win_core::tree::NodeKind;
use i3win_core::wm::Wm;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, DrawTextW, GetDC, GetTextExtentPoint32W, InvalidateRect, ReleaseDC,
    SelectObject, SetBkMode, SetTextColor, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::bar_layout::{click_command, hit_test, layout_bar, BarLayout, ItemKind};
use crate::mainloop::{self, MainMsg};
use crate::win::gdi;

/// Everything a bar window needs to paint and hit-test itself.
#[derive(Clone, Debug, PartialEq)]
pub struct BarData {
    pub layout: BarLayout,
    pub background: Color,
    pub separator: Color,
    pub font: Font,
    pub scale: f32,
    pub blocks: Vec<Block>,
}

struct BarWindow {
    hwnd: HWND,
    rect: Rect,
    data: BarData,
    /// Whether the window is currently shown.
    shown: bool,
}

pub struct Bars {
    windows: HashMap<String, BarWindow>,
}

/// The bar strip inside output rect `o`.
pub fn bar_rect(o: Rect, position: BarPosition, h: i32) -> Rect {
    match position {
        BarPosition::Top => Rect::new(o.x, o.y, o.w, h),
        BarPosition::Bottom => Rect::new(o.x, o.bottom() - h, o.w, h),
    }
}

fn register_class() {
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

fn create_window(r: Rect, data: &BarData) -> Option<HWND> {
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

fn set_data(hwnd: HWND, data: &BarData) {
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

/// Width in pixels of `text` in `font` at `scale`, measured on a screen DC.
fn measure_text(font: &Font, scale: f32, text: &str) -> i32 {
    let hfont = gdi::font(&font.family, font.size_pt, scale);
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    // SAFETY: the screen DC is released and the previous font restored.
    unsafe {
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            return 0;
        }
        let old = SelectObject(hdc, hfont);
        let ok = GetTextExtentPoint32W(hdc, &wide, &mut size).as_bool();
        SelectObject(hdc, old);
        ReleaseDC(None, hdc);
        if ok {
            size.cx
        } else {
            0
        }
    }
}

impl Default for Bars {
    fn default() -> Self {
        Self::new()
    }
}

impl Bars {
    pub fn new() -> Bars {
        register_class();
        Bars {
            windows: HashMap::new(),
        }
    }

    /// Brings the bar windows in line with the config, outputs and `blocks`.
    pub fn update(&mut self, wm: &Wm, arr: &Arrangement, blocks: &[Block]) {
        let Some(bar) = wm.config.bar.as_ref() else {
            self.destroy_all();
            return;
        };
        let font = bar.font.clone().unwrap_or_else(|| wm.config.font.clone());
        let mode = (wm.mode != "default").then_some(wm.mode.as_str());
        let mut live: HashSet<String> = HashSet::new();
        for out in wm.outputs() {
            let NodeKind::Output { name, rect, scale } = &wm.tree.get(out).kind else {
                continue;
            };
            let (scale, o) = (*scale, *rect);
            let Some(h) = bar_height(&wm.config, scale) else {
                continue;
            };
            live.insert(name.clone());
            let r = bar_rect(o, bar.position, h);
            let buttons = workspace_buttons(wm, out);
            let layout = layout_bar(
                r.w,
                r.h,
                scale,
                &buttons,
                bar.workspace_buttons,
                mode,
                blocks,
                &bar.colors,
                &|t| measure_text(&font, scale, t),
            );
            let data = BarData {
                layout,
                background: bar.colors.background,
                separator: bar.colors.separator,
                font: font.clone(),
                scale,
                blocks: blocks.to_vec(),
            };
            let mut created = false;
            if let Some(bw) = self.windows.get_mut(name) {
                if bw.data != data {
                    set_data(bw.hwnd, &data);
                    bw.data = data;
                }
            } else {
                let Some(hwnd) = create_window(r, &data) else {
                    continue;
                };
                created = true;
                self.windows.insert(
                    name.clone(),
                    BarWindow {
                        hwnd,
                        rect: r,
                        data,
                        shown: false,
                    },
                );
            }
            if let Some(bw) = self.windows.get_mut(name) {
                let want_shown = !arr.fullscreen_outputs.contains(name);
                // Re-asserting topmost every flush would hop over menus and tooltips.
                let raise = created || bw.rect != r || (want_shown && !bw.shown);
                // SAFETY: plain Win32 calls on our own window.
                unsafe {
                    if raise {
                        let _ = SetWindowPos(
                            bw.hwnd,
                            HWND_TOPMOST,
                            r.x,
                            r.y,
                            r.w.max(1),
                            r.h.max(1),
                            SWP_NOACTIVATE,
                        );
                        bw.rect = r;
                    }
                    if want_shown != bw.shown {
                        let _ = ShowWindow(bw.hwnd, if want_shown { SW_SHOWNA } else { SW_HIDE });
                        bw.shown = want_shown;
                    }
                }
            }
        }
        let stale: Vec<String> = self
            .windows
            .keys()
            .filter(|k| !live.contains(*k))
            .cloned()
            .collect();
        for k in stale {
            if let Some(bw) = self.windows.remove(&k) {
                // SAFETY: destroying our own window.
                unsafe {
                    let _ = DestroyWindow(bw.hwnd);
                }
            }
        }
    }

    /// Destroys every bar window.
    pub fn destroy_all(&mut self) {
        for (_, bw) in self.windows.drain() {
            // SAFETY: destroying our own window.
            unsafe {
                let _ = DestroyWindow(bw.hwnd);
            }
        }
    }
}

thread_local! {
    /// Wheel delta not yet worth a full notch (touchpads send small deltas).
    static WHEEL_ACC: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Adds `delta` to the accumulator and returns whole notches: positive is wheel
/// up (previous workspace), negative is down. The remainder stays in `acc`.
fn wheel_steps(acc: &mut i32, delta: i32) -> i32 {
    const WHEEL_DELTA: i32 = 120;
    *acc += delta;
    let steps = *acc / WHEEL_DELTA;
    *acc -= steps * WHEEL_DELTA;
    steps
}

fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

fn paint(hwnd: HWND, data: &BarData) {
    let font = gdi::font(&data.font.family, data.font.size_pt, data.scale);
    let s3 = (3.0 * data.scale).round() as i32;
    gdi::paint_buffered(hwnd, |hdc, client| {
        gdi::fill(hdc, client, data.background);
        for item in &data.layout.items {
            if let Some(bg) = item.bg {
                gdi::fill(hdc, item.rect, bg);
            }
            if let Some(b) = item.border {
                gdi::outline(hdc, item.rect, b);
            }
            let mut wide: Vec<u16> = item.text.encode_utf16().collect();
            let mut rc = to_rect(item.rect);
            // SAFETY: `wide` and `rc` outlive the calls; the old font is restored.
            unsafe {
                let old = SelectObject(hdc, font);
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, gdi::colorref(item.fg));
                DrawTextW(
                    hdc,
                    &mut wide,
                    &mut rc,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX,
                );
                SelectObject(hdc, old);
            }
        }
        for &x in &data.layout.separators {
            gdi::fill(
                hdc,
                Rect::new(x, s3, 1, (client.h - 2 * s3).max(0)),
                data.separator,
            );
        }
    });
}

fn click(hwnd: HWND, data: &BarData, button: u8, lp: LPARAM) {
    let x = (lp.0 & 0xffff) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
    let Some(item) = hit_test(&data.layout, x, y) else {
        return;
    };
    match &item.kind {
        ItemKind::Workspace(name) => {
            if button == 1 {
                mainloop::post(MainMsg::RunCommand(click_command(name)));
            }
        }
        ItemKind::Mode => {}
        ItemKind::Block(i) => {
            let mut pt = POINT { x, y };
            // SAFETY: plain Win32 call on our own window.
            unsafe {
                let _ = ClientToScreen(hwnd, &mut pt);
            }
            let block = data.blocks.get(*i);
            mainloop::post(MainMsg::StatusClick {
                name: block.and_then(|b| b.name.clone()),
                instance: block.and_then(|b| b.instance.clone()),
                button,
                x: pt.x,
                y: pt.y,
            });
        }
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
        WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_RBUTTONDOWN => {
            let button = match msg {
                WM_LBUTTONDOWN => 1,
                WM_MBUTTONDOWN => 2,
                _ => 3,
            };
            if let Some(data) = ptr.as_ref() {
                click(hwnd, data, button, lp);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xffff) as i16 as i32;
            let steps = WHEEL_ACC.with(|a| {
                let mut acc = a.get();
                let n = wheel_steps(&mut acc, delta);
                a.set(acc);
                n
            });
            let cmd = if steps > 0 {
                "workspace prev"
            } else {
                "workspace next"
            };
            for _ in 0..steps.unsigned_abs() {
                mainloop::post(MainMsg::RunCommand(cmd.into()));
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
    #[test]
    fn wheel_accumulates_small_deltas() {
        let mut acc = 0;
        assert_eq!(wheel_steps(&mut acc, 40), 0);
        assert_eq!(wheel_steps(&mut acc, 40), 0);
        assert_eq!(wheel_steps(&mut acc, 40), 1);
        assert_eq!(acc, 0);
    }
    #[test]
    fn wheel_down_and_multi_notch() {
        let mut acc = 0;
        assert_eq!(wheel_steps(&mut acc, -120), -1);
        assert_eq!(wheel_steps(&mut acc, 240), 2);
        assert_eq!(wheel_steps(&mut acc, 100), 0);
        assert_eq!(acc, 100);
    }
    #[test]
    fn top_and_bottom_strips() {
        let o = Rect::new(100, 50, 800, 600);
        assert_eq!(
            bar_rect(o, BarPosition::Top, 20),
            Rect::new(100, 50, 800, 20)
        );
        assert_eq!(
            bar_rect(o, BarPosition::Bottom, 20),
            Rect::new(100, 630, 800, 20)
        );
    }
}
