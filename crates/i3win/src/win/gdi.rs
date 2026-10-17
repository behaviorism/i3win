//! Small GDI helpers shared by the decoration windows.
use std::cell::RefCell;
use std::collections::HashMap;

use i3win_core::arrange::DecoState;
use i3win_core::config::{resolve_font_family, ClientColors, Color, ColorClass};
use i3win_core::geom::Rect;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// `0x00BBGGRR`.
pub fn colorref(c: Color) -> COLORREF {
    COLORREF(c.0 as u32 | (c.1 as u32) << 8 | (c.2 as u32) << 16)
}

/// The color class used to draw a decoration in `state`.
pub fn colors_for(state: DecoState, cc: &ClientColors) -> ColorClass {
    match state {
        DecoState::Focused => cc.focused,
        DecoState::FocusedInactive => cc.focused_inactive,
        DecoState::Unfocused => cc.unfocused,
        DecoState::Urgent => cc.urgent,
    }
}

type FontKey = (String, i32, i32);

thread_local! {
    /// Created fonts as raw handle values, freed by `free_fonts`.
    static FONTS: RefCell<HashMap<FontKey, isize>> = RefCell::new(HashMap::new());
}

/// A cached font for `family` at `size_pt` scaled by `scale`.
pub fn font(family: &str, size_pt: f32, scale: f32) -> HFONT {
    let key = (
        family.to_string(),
        (size_pt * 10.0) as i32,
        (scale * 100.0) as i32,
    );
    FONTS.with(|f| {
        let mut map = f.borrow_mut();
        if let Some(&h) = map.get(&key) {
            return HFONT(h as *mut _);
        }
        let height = -((size_pt * 96.0 / 72.0 * scale).round() as i32);
        let mut face: Vec<u16> = resolve_font_family(family).encode_utf16().collect();
        face.truncate(31);
        face.push(0);
        // SAFETY: `face` is NUL-terminated and outlives the call.
        let h = unsafe {
            CreateFontW(
                height,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                CLEARTYPE_QUALITY.0 as u32,
                0,
                PCWSTR(face.as_ptr()),
            )
        };
        if !h.is_invalid() {
            map.insert(key, h.0 as isize);
        }
        h
    })
}

/// Deletes every cached font.
pub fn free_fonts() {
    FONTS.with(|f| {
        for (_, h) in f.borrow_mut().drain() {
            // SAFETY: the handle came from CreateFontW and is no longer in use.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(h as *mut _));
            }
        }
    });
}

fn to_rect(r: Rect) -> RECT {
    RECT {
        left: r.x,
        top: r.y,
        right: r.right(),
        bottom: r.bottom(),
    }
}

/// Fills `r` with `c`.
pub fn fill(hdc: HDC, r: Rect, c: Color) {
    // SAFETY: the brush is created and deleted here.
    unsafe {
        let b = CreateSolidBrush(colorref(c));
        FillRect(hdc, &to_rect(r), b);
        let _ = DeleteObject(b);
    }
}

/// Draws a 1 px outline just inside `r`.
pub fn outline(hdc: HDC, r: Rect, c: Color) {
    // SAFETY: the brush is created and deleted here.
    unsafe {
        let b = CreateSolidBrush(colorref(c));
        FrameRect(hdc, &to_rect(r), b);
        let _ = DeleteObject(b);
    }
}

/// Draws one line of `text` in `r` with `font`, left aligned and vertically centered.
pub fn draw_text(hdc: HDC, r: Rect, text: &str, c: Color, font: HFONT) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rc = to_rect(r);
    // SAFETY: `wide` and `rc` outlive the calls; the old font is restored.
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, colorref(c));
        DrawTextW(
            hdc,
            &mut wide,
            &mut rc,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | DT_LEFT,
        );
        SelectObject(hdc, old);
    }
}

/// Double-buffered paint of `hwnd`'s client area; `f` draws into the memory DC.
pub fn paint_buffered(hwnd: HWND, f: impl FnOnce(HDC, Rect)) {
    // SAFETY: standard BeginPaint/EndPaint pairing; every GDI object is released.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w > 0 && h > 0 && !hdc.is_invalid() {
            let mem = CreateCompatibleDC(hdc);
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            if !mem.is_invalid() && !bmp.is_invalid() {
                let old = SelectObject(mem, bmp);
                f(mem, Rect::new(0, 0, w, h));
                let _ = BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
                SelectObject(mem, old);
            }
            if !bmp.is_invalid() {
                let _ = DeleteObject(bmp);
            }
            if !mem.is_invalid() {
                let _ = DeleteDC(mem);
            }
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use i3win_core::arrange::DecoState;
    use i3win_core::config::Color;
    #[test]
    fn colorref_packs_bgr() {
        assert_eq!(colorref(Color(0x11, 0x22, 0x33)).0, 0x0033_2211);
    }
    #[test]
    fn state_to_color_class() {
        let cc = i3win_core::config::Config::default().colors;
        assert_eq!(colors_for(DecoState::Urgent, &cc), cc.urgent);
        assert_eq!(
            colors_for(DecoState::FocusedInactive, &cc),
            cc.focused_inactive
        );
    }
}
