//! Bar windows: one topmost, never-activating GDI strip per output.
//!
//! The window procedure never touches `APP`: it paints from the `BarData`
//! stored in `GWLP_USERDATA` and only posts `MainMsg`s on clicks and wheel turns.
use std::collections::{HashMap, HashSet};

use i3win_core::arrange::{bar_applies, bar_height_of, Arrangement};
use i3win_core::bar_model::workspace_buttons;
use i3win_core::config::{BarMode, BarPosition, Font};
use i3win_core::geom::Rect;
use i3win_core::i3bar::{Block, Click};
use i3win_core::markup::parse_markup;
use i3win_core::tree::NodeKind;
use i3win_core::wm::Wm;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetDC, GetMonitorInfoW, GetTextExtentPoint32W, InvalidateRect,
    MonitorFromWindow, ReleaseDC, SelectObject, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::bar_layout::{click_command, hit_test, layout_bar, BarLayout, BarStyle, ItemKind};
use crate::mainloop::{self, MainMsg};
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
}

struct BarWindow {
    hwnd: HWND,
    rect: Rect,
    data: BarData,
    /// Whether the window is currently shown.
    shown: bool,
}

pub struct Bars {
    /// Keyed by (bar id, output name).
    windows: HashMap<(String, String), BarWindow>,
}

/// The bar strip inside output rect `o`, `offset` pixels in from the edge
/// (the summed heights of the earlier bars at the same edge).
pub fn bar_rect_stacked(o: Rect, position: BarPosition, offset: i32, h: i32) -> Rect {
    match position {
        BarPosition::Top => Rect::new(o.x, o.y + offset, o.w, h),
        BarPosition::Bottom => Rect::new(o.x, o.bottom() - offset - h, o.w, h),
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
/// With `markup`, `text` is parsed and measured run by run.
fn measure_text(font: &Font, scale: f32, text: &str, markup: bool) -> i32 {
    // SAFETY: the screen DC is released and the previous font restored.
    unsafe {
        let hdc = GetDC(None);
        if hdc.is_invalid() {
            return 0;
        }
        let width = if markup {
            gdi::measure_runs(hdc, &parse_markup(text), font, scale)
        } else {
            let hfont = gdi::font(&font.family, font.size_pt, scale);
            let wide: Vec<u16> = text.encode_utf16().collect();
            let mut size = SIZE::default();
            let old = SelectObject(hdc, hfont);
            let ok = GetTextExtentPoint32W(hdc, &wide, &mut size).as_bool();
            SelectObject(hdc, old);
            if ok {
                size.cx
            } else {
                0
            }
        };
        ReleaseDC(None, hdc);
        width
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

    /// Brings the bar windows in line with the config, outputs and `blocks`
    /// (latest status blocks per bar id). Only Dock bars get windows for now.
    pub fn update(&mut self, wm: &Wm, arr: &Arrangement, blocks: &HashMap<String, Vec<Block>>) {
        let mode = (wm.mode != "default").then_some(wm.mode.as_str());
        let mut live: HashSet<(String, String)> = HashSet::new();
        let no_blocks: Vec<Block> = Vec::new();
        for out in wm.outputs() {
            let NodeKind::Output {
                name,
                rect,
                scale,
                primary,
            } = &wm.tree.get(out).kind
            else {
                continue;
            };
            let (scale, o) = (*scale, *rect);
            // Heights already taken at the top and bottom edges by earlier bars.
            let (mut top, mut bottom) = (0, 0);
            for bar in &wm.config.bars {
                if bar.mode != BarMode::Dock || !bar_applies(bar, name, *primary) {
                    continue;
                }
                let h = bar_height_of(bar, &wm.config, scale);
                let offset = match bar.position {
                    BarPosition::Top => &mut top,
                    BarPosition::Bottom => &mut bottom,
                };
                let r = bar_rect_stacked(o, bar.position, *offset, h);
                *offset += h;
                let font = bar.font.clone().unwrap_or_else(|| wm.config.font.clone());
                let key = (bar.id.clone(), name.clone());
                live.insert(key.clone());
                let bar_blocks = blocks.get(&bar.id).unwrap_or(&no_blocks);
                let buttons = workspace_buttons(wm, out);
                let style = BarStyle {
                    colors: &bar.colors,
                    focused_output: buttons.iter().any(|b| b.focused),
                    show_buttons: bar.workspace_buttons,
                    show_mode: bar.binding_mode_indicator,
                    strip_numbers: bar.strip_workspace_numbers,
                    strip_name: bar.strip_workspace_name,
                    workspace_min_width: bar.workspace_min_width,
                    separator_symbol: bar.separator_symbol.as_deref(),
                    mode_markup: wm.config.mode_markup.contains(&wm.mode),
                    padding: bar.padding,
                };
                let layout = layout_bar(
                    r.w,
                    r.h,
                    scale,
                    &buttons,
                    mode,
                    bar_blocks,
                    &style,
                    &|t, markup| measure_text(&font, scale, t, markup),
                );
                let data = BarData {
                    bar_id: bar.id.clone(),
                    layout,
                    font: font.clone(),
                    scale,
                    blocks: bar_blocks.clone(),
                };
                let mut created = false;
                if let Some(bw) = self.windows.get_mut(&key) {
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
                        key.clone(),
                        BarWindow {
                            hwnd,
                            rect: r,
                            data,
                            shown: false,
                        },
                    );
                }
                if let Some(bw) = self.windows.get_mut(&key) {
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
                            let _ =
                                ShowWindow(bw.hwnd, if want_shown { SW_SHOWNA } else { SW_HIDE });
                            bw.shown = want_shown;
                        }
                    }
                }
            }
        }
        let stale: Vec<(String, String)> = self
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

    /// The workspace whose button lies under screen point `(x, y)` on a
    /// visible bar, if any (drop target for dragging windows onto buttons).
    #[allow(dead_code)] // consumed by the drag-and-drop plan
    pub fn workspace_at(&self, x: i32, y: i32) -> Option<String> {
        self.windows.values().filter(|bw| bw.shown).find_map(|bw| {
            let r = bw.rect;
            if x < r.x || x >= r.right() || y < r.y || y >= r.bottom() {
                return None;
            }
            match &hit_test(&bw.data.layout, x - r.x, y - r.y)?.kind {
                ItemKind::Workspace(name) => Some(name.clone()),
                _ => None,
            }
        })
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

fn paint(hwnd: HWND, data: &BarData) {
    let font = gdi::font(&data.font.family, data.font.size_pt, data.scale);
    let s3 = (3.0 * data.scale).round() as i32;
    gdi::paint_buffered(hwnd, |hdc, client| {
        gdi::fill(hdc, client, data.layout.background);
        for item in &data.layout.items {
            let r = item.rect;
            if let Some(bg) = item.bg {
                gdi::fill(hdc, r, bg);
            }
            if let Some(b) = item.border {
                let [l, t, rt, bt] = item.borders;
                gdi::fill(hdc, Rect::new(r.x, r.y, l, r.h), b);
                gdi::fill(hdc, Rect::new(r.right() - rt, r.y, rt, r.h), b);
                gdi::fill(hdc, Rect::new(r.x, r.y, r.w, t), b);
                gdi::fill(hdc, Rect::new(r.x, r.bottom() - bt, r.w, bt), b);
            }
            let text_area = Rect::new(
                item.text_x,
                r.y,
                (r.right() - item.borders[2] - item.text_x).max(0),
                r.h,
            );
            if item.markup {
                gdi::draw_runs(
                    hdc,
                    item.text_x,
                    r.inset(
                        item.borders[0],
                        item.borders[1],
                        item.borders[2],
                        item.borders[3],
                    ),
                    &parse_markup(&item.text),
                    item.fg,
                    &data.font,
                    data.scale,
                );
            } else {
                gdi::draw_text(hdc, text_area, &item.text, item.fg, font);
            }
        }
        let content = data.layout.content;
        for sep in &data.layout.separators {
            let color = data.layout.separator_color;
            match &sep.symbol {
                Some(sym) => {
                    let w = measure_text(&data.font, data.scale, sym, false);
                    let area = Rect::new(sep.x - w / 2, content.y, w, content.h);
                    gdi::draw_text(hdc, area, sym, color, font);
                }
                None => gdi::fill(
                    hdc,
                    Rect::new(sep.x, content.y + s3, 1, (content.h - 2 * s3).max(0)),
                    color,
                ),
            }
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
            let (mut ox, mut oy) = (0, 0);
            // SAFETY: plain Win32 calls on our own window; `mi` is sized correctly.
            unsafe {
                let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                if GetMonitorInfoW(mon, &mut mi).as_bool() {
                    ox = mi.rcMonitor.left;
                    oy = mi.rcMonitor.top;
                }
            }
            let key_down = |vk: VIRTUAL_KEY| {
                // SAFETY: plain Win32 call.
                unsafe { GetKeyState(vk.0 as i32) < 0 }
            };
            let mut modifiers = Vec::new();
            if key_down(VK_SHIFT) {
                modifiers.push("Shift".to_string());
            }
            if key_down(VK_CONTROL) {
                modifiers.push("Control".to_string());
            }
            if key_down(VK_MENU) {
                modifiers.push("Mod1".to_string());
            }
            if key_down(VK_LWIN) || key_down(VK_RWIN) {
                modifiers.push("Mod4".to_string());
            }
            mainloop::post(MainMsg::StatusClick {
                bar: data.bar_id.clone(),
                click: Click {
                    name: block.and_then(|b| b.name.clone()),
                    instance: block.and_then(|b| b.instance.clone()),
                    button,
                    modifiers,
                    x: pt.x,
                    y: pt.y,
                    relative_x: x - item.rect.x,
                    relative_y: y - item.rect.y,
                    output_x: pt.x - ox,
                    output_y: pt.y - oy,
                    width: item.rect.w,
                    height: item.rect.h,
                },
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
            bar_rect_stacked(o, BarPosition::Top, 0, 20),
            Rect::new(100, 50, 800, 20)
        );
        assert_eq!(
            bar_rect_stacked(o, BarPosition::Bottom, 0, 20),
            Rect::new(100, 630, 800, 20)
        );
    }
    #[test]
    fn bar_rect_stacking() {
        let o = Rect::new(0, 0, 1000, 500);
        assert_eq!(
            bar_rect_stacked(o, BarPosition::Bottom, 0, 20),
            Rect::new(0, 480, 1000, 20)
        );
        assert_eq!(
            bar_rect_stacked(o, BarPosition::Bottom, 20, 20),
            Rect::new(0, 460, 1000, 20)
        );
        assert_eq!(
            bar_rect_stacked(o, BarPosition::Top, 20, 20),
            Rect::new(0, 20, 1000, 20)
        );
    }
    #[test]
    fn bar_outputs_filter() {
        let c = i3win_core::config::parse_config("bar {\noutput NOPE\n}\n");
        assert!(!bar_applies(&c.bars[0], "DISPLAY1", true));
    }
}
