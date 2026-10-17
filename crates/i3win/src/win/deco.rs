//! Decoration windows: title bars, borders and tab/stack bars drawn with GDI.
//!
//! The window procedure never touches `APP`: it paints from the `DecoData`
//! stored in `GWLP_USERDATA` and only posts `MainMsg::FocusNode` on clicks.
//! A left-button drag on a title bar or tab is captured here and fed into the
//! mouse event path (`mouse::post_event`), where the App's drag machine takes it.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use i3win_core::arrange::{Arrangement, Deco, Tab};
use i3win_core::config::{ClientColors, Color, Config, Font};
use i3win_core::geom::Rect;
use i3win_core::markup::parse_markup;
use i3win_core::tree::{NodeId, WindowId};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, HBRUSH, HDC, HFONT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::mainloop::{self, MainMsg};
use crate::win::gdi;
use crate::win::mouse::{self, Button, MouseEvent};

/// Pointer travel (px, either axis) that turns a title-bar click into a drag.
const DRAG_THRESHOLD: i32 = 4;

/// A left-button press on a title bar or tab, held with the mouse captured.
#[derive(Clone, Copy, Debug)]
struct TitleDrag {
    hwnd: isize,
    node: NodeId,
    origin: (i32, i32),
    /// Past the threshold: a `TitlePress` was posted.
    started: bool,
}

thread_local! {
    static TITLE_DRAG: Cell<Option<TitleDrag>> = const { Cell::new(None) };
}

/// Whether the pointer moved far enough from `origin` to start a drag.
fn past_threshold(origin: (i32, i32), x: i32, y: i32) -> bool {
    (x - origin.0).abs() > DRAG_THRESHOLD || (y - origin.1).abs() > DRAG_THRESHOLD
}

fn contains(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

fn cursor() -> Option<(i32, i32)> {
    let mut pt = POINT::default();
    // SAFETY: pt is a valid out pointer for the synchronous call.
    unsafe { GetCursorPos(&mut pt).is_ok().then_some((pt.x, pt.y)) }
}

/// Ends a captured title-bar press without posting anything (the App cancelled or
/// finished the drag itself).
pub fn end_title_drag() {
    if TITLE_DRAG.with(|t| t.take()).is_some() {
        // SAFETY: releasing our own thread's capture.
        unsafe {
            let _ = ReleaseCapture();
        }
    }
}

/// The captured press of `hwnd` ended: on button-up (`released`) a started drag
/// gets its Release; on capture lost to someone else (or the deco destroyed) it
/// is cancelled.
fn finish_title_drag(hwnd: HWND, released: bool) {
    let Some(d) = TITLE_DRAG
        .with(|t| t.get())
        .filter(|d| d.hwnd == hwnd.0 as isize)
    else {
        return;
    };
    TITLE_DRAG.with(|t| t.set(None));
    if d.started {
        let ev = if released {
            let (x, y) = cursor().unwrap_or(d.origin);
            MouseEvent::Release {
                button: Button::Left,
                x,
                y,
            }
        } else {
            MouseEvent::Cancel
        };
        mouse::post_event(ev);
    }
}

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

thread_local! {
    /// WM_GETICON(ICON_SMALL2) results by window, filled by `icon_callback`.
    static ICON_CACHE: RefCell<HashMap<isize, isize>> = RefCell::new(HashMap::new());
    /// Windows we already asked for an ICON_SMALL2.
    static ICON_REQUESTED: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
}

unsafe extern "system" fn icon_callback(hwnd: HWND, _msg: u32, deco: usize, result: LRESULT) {
    if result.0 != 0 {
        ICON_CACHE.with(|c| c.borrow_mut().insert(hwnd.0 as isize, result.0));
        let _ = InvalidateRect(HWND(deco as *mut _), None, false);
    }
}

/// The small icon of `id`: the cached ICON_SMALL2, else the class's small or big icon.
/// Requests ICON_SMALL2 asynchronously once per window.
fn window_icon(id: WindowId, deco: HWND) -> Option<HICON> {
    let hwnd = HWND(id.0 as *mut _);
    if let Some(h) = ICON_CACHE.with(|c| c.borrow().get(&id.0).copied()) {
        return Some(HICON(h as *mut _));
    }
    if ICON_REQUESTED.with(|r| r.borrow_mut().insert(id.0)) {
        // SAFETY: the callback only touches thread-locals and invalidates our deco window.
        unsafe {
            let _ = SendMessageCallbackW(
                hwnd,
                WM_GETICON,
                WPARAM(2), // ICON_SMALL2
                LPARAM(0),
                Some(icon_callback),
                deco.0 as usize,
            );
        }
    }
    // SAFETY: plain class-long reads on a foreign window; failure yields 0.
    let raw = unsafe {
        match GetClassLongPtrW(hwnd, GCLP_HICONSM) {
            0 => GetClassLongPtrW(hwnd, GCLP_HICON),
            v => v,
        }
    };
    (raw != 0).then_some(HICON(raw as *mut _))
}

/// Draws a title as markup runs or plain text.
fn draw_title(
    hdc: HDC,
    r: Rect,
    title: &str,
    markup: bool,
    fg: Color,
    font: HFONT,
    data: &DecoData,
) {
    if markup {
        gdi::draw_runs(
            hdc,
            r.x,
            r,
            &parse_markup(title),
            fg,
            &data.font,
            data.scale,
        );
    } else {
        gdi::draw_text(hdc, r, title, fg, font);
    }
}

fn paint(hwnd: HWND, data: &DecoData) {
    let font = gdi::font(&data.font.family, data.font.size_pt, data.scale);
    let inset = (4.0 * data.scale).round() as i32;
    gdi::paint_buffered(hwnd, |hdc, client| match &data.deco {
        Deco::Frame {
            title_rect,
            title,
            markup,
            state,
            icon,
            ..
        } => {
            let cc = gdi::colors_for(*state, &data.colors);
            // The border lines are whatever the client window does not cover:
            // each side is as wide as `borders` says, hidden sides are covered
            // by the client itself.
            gdi::fill(hdc, client, cc.child_border);
            if let Some(t) = title_rect {
                let area = Rect::new(0, 0, client.w, t.h);
                gdi::fill(hdc, area, cc.background);
                gdi::outline(hdc, area, cc.border);
                let mut text = area.inset(inset, 0, inset, 0);
                if let Some((id, padding)) = icon {
                    let size = (t.h - 4).max(0);
                    if let Some(h) = window_icon(*id, hwnd).filter(|_| size > 0) {
                        let y = (t.h - size) / 2;
                        // SAFETY: drawing a live icon handle into our paint DC.
                        unsafe {
                            let _ = DrawIconEx(
                                hdc,
                                text.x,
                                y,
                                h,
                                size,
                                size,
                                0,
                                HBRUSH::default(),
                                DI_NORMAL,
                            );
                        }
                        let pad = (*padding as f32 * data.scale).round() as i32;
                        let used = size + pad;
                        text = text.inset(used, 0, 0, 0);
                    }
                }
                draw_title(hdc, text, title, *markup, cc.text, font, data);
            }
        }
        Deco::TabBar { rect, tabs, .. } => {
            for t in tabs {
                let cc = gdi::colors_for(t.state, &data.colors);
                let r = Rect::new(t.rect.x - rect.x, t.rect.y - rect.y, t.rect.w, t.rect.h);
                gdi::fill(hdc, r, cc.background);
                gdi::outline(hdc, r, cc.border);
                draw_title(
                    hdc,
                    r.inset(inset, 0, inset, 0),
                    &t.title,
                    t.markup,
                    cc.text,
                    font,
                    data,
                );
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
                let pt = cursor();
                // The node a drag from this press would move: a title bar or a tab.
                let drag_node = match &data.deco {
                    Deco::Frame {
                        key, title_rect, ..
                    } => {
                        mainloop::post(MainMsg::FocusNode(*key));
                        title_rect
                            .zip(pt)
                            .filter(|(t, (x, y))| contains(*t, *x, *y))
                            .map(|_| *key)
                    }
                    Deco::TabBar { tabs, rect, .. } => {
                        let n = pt.and_then(|(x, y)| tab_at(tabs, (rect.x, rect.y), x, y));
                        if let Some(n) = n {
                            mainloop::post(MainMsg::FocusNode(n));
                        }
                        n
                    }
                };
                if let (Some(node), Some(origin)) = (drag_node, pt) {
                    TITLE_DRAG.with(|t| {
                        t.set(Some(TitleDrag {
                            hwnd: hwnd.0 as isize,
                            node,
                            origin,
                            started: false,
                        }))
                    });
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let held = TITLE_DRAG
                .with(|t| t.get())
                .filter(|d| d.hwnd == hwnd.0 as isize && GetCapture() == hwnd);
            if let (Some(mut d), Some((x, y))) = (held, cursor()) {
                if !d.started && past_threshold(d.origin, x, y) {
                    d.started = true;
                    TITLE_DRAG.with(|t| t.set(Some(d)));
                    mouse::post_event(MouseEvent::TitlePress {
                        x: d.origin.0,
                        y: d.origin.1,
                        node: d.node,
                    });
                }
                if d.started {
                    mouse::post_move(x, y);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            finish_title_drag(hwnd, true);
            // Releasing capture sends WM_CAPTURECHANGED, which finds nothing left.
            if GetCapture() == hwnd {
                let _ = ReleaseCapture();
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            finish_title_drag(hwnd, false);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            finish_title_drag(hwnd, false);
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
                markup: false,
                state: DecoState::Unfocused,
                rect: Rect::new(100, 0, 50, 20),
                node: NodeId(1),
            },
            Tab {
                title: "b".into(),
                markup: false,
                state: DecoState::Focused,
                rect: Rect::new(150, 0, 50, 20),
                node: NodeId(2),
            },
        ];
        assert_eq!(tab_at(&tabs, (100, 0), 160, 10), Some(NodeId(2)));
        assert_eq!(tab_at(&tabs, (100, 0), 99, 10), None);
    }
    #[test]
    fn title_drag_threshold() {
        assert!(!past_threshold((100, 100), 104, 96));
        assert!(past_threshold((100, 100), 105, 100));
        assert!(past_threshold((100, 100), 100, 95));
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
