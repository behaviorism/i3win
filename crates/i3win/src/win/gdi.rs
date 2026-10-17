//! Small GDI helpers shared by the decoration windows.
use std::cell::RefCell;
use std::collections::HashMap;

use i3win_core::arrange::DecoState;
use i3win_core::config::{resolve_font_family, ClientColors, Color, ColorClass, Font};
use i3win_core::geom::Rect;
use i3win_core::markup::{Run, Style};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, RECT, SIZE};
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

/// family, size x10, scale x100, weight, italic, underline, strike.
type FontKey = (String, i32, i32, i32, bool, bool, bool);

thread_local! {
    /// Created fonts as raw handle values, freed by `free_fonts`.
    static FONTS: RefCell<HashMap<FontKey, isize>> = RefCell::new(HashMap::new());
}

/// A cached regular-weight font for `family` at `size_pt` scaled by `scale`.
pub fn font(family: &str, size_pt: f32, scale: f32) -> HFONT {
    font_ex(family, size_pt, scale, 400, false, false, false)
}

/// A cached font with explicit weight and style flags.
pub fn font_ex(
    family: &str,
    size_pt: f32,
    scale: f32,
    weight: i32,
    italic: bool,
    underline: bool,
    strike: bool,
) -> HFONT {
    let key = (
        family.to_string(),
        (size_pt * 10.0) as i32,
        (scale * 100.0) as i32,
        weight,
        italic,
        underline,
        strike,
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
                weight,
                italic as u32,
                underline as u32,
                strike as u32,
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

/// Family, point size and weight for one run on top of the `base` font.
/// Non-finite or non-positive `size_pt`/`scale` are ignored; the size is clamped to 1..=200.
pub fn run_font_spec(base: &Font, style: &Style) -> (String, f32, i32) {
    let valid = |v: &f32| v.is_finite() && *v > 0.0;
    let family = match &style.family {
        Some(f) if !f.is_empty() => f.clone(),
        _ if style.mono => "Consolas".to_string(),
        _ => base.family.clone(),
    };
    let scale = Some(style.scale).filter(valid).unwrap_or(1.0);
    let size = style.size_pt.filter(valid).unwrap_or(base.size_pt * scale);
    let size = if size.is_finite() {
        size.clamp(1.0, 200.0)
    } else {
        base.size_pt.clamp(1.0, 200.0)
    };
    let weight = style
        .weight
        .map(|w| i32::from(w).clamp(100, 900))
        .unwrap_or(if style.bold { 700 } else { 400 });
    (family, size, weight)
}

fn run_font(base: &Font, style: &Style, scale: f32) -> HFONT {
    let (family, size, weight) = run_font_spec(base, style);
    font_ex(
        &family,
        size,
        scale,
        weight,
        style.italic,
        style.underline,
        style.strike,
    )
}

/// One run measured with its font.
struct Laid {
    font: HFONT,
    wide: Vec<u16>,
    w: i32,
    asc: i32,
    h: i32,
}

fn lay_out(hdc: HDC, run: &Run, base: &Font, scale: f32) -> Laid {
    let font = run_font(base, &run.style, scale);
    let wide: Vec<u16> = run.text.encode_utf16().collect();
    let mut size = SIZE::default();
    let mut tm = TEXTMETRICW::default();
    // SAFETY: out-pointers are valid locals; the previous font is restored.
    unsafe {
        let old = SelectObject(hdc, font);
        let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
        let _ = GetTextMetricsW(hdc, &mut tm);
        SelectObject(hdc, old);
    }
    Laid {
        font,
        wide,
        w: size.cx.max(0),
        asc: tm.tmAscent.max(0),
        h: tm.tmHeight.max(0),
    }
}

/// Total width in pixels of `runs` drawn with `base` at UI `scale`.
pub fn measure_runs(hdc: HDC, runs: &[Run], base: &Font, scale: f32) -> i32 {
    runs.iter().fold(0i32, |acc, run| {
        acc.saturating_add(lay_out(hdc, run, base, scale).w)
    })
}

/// Draws `runs` starting at `x`, vertically centered in `r` (runs share a
/// baseline) and clipped to `r`. A run's `bg` fills its text extent first.
pub fn draw_runs(
    hdc: HDC,
    x: i32,
    r: Rect,
    runs: &[Run],
    default_fg: Color,
    base: &Font,
    scale: f32,
) {
    let laid: Vec<Laid> = runs
        .iter()
        .map(|run| lay_out(hdc, run, base, scale))
        .collect();
    let ascent = laid.iter().map(|l| l.asc).max().unwrap_or(0);
    let descent = laid.iter().map(|l| l.h - l.asc).max().unwrap_or(0);
    let top = r.y + (r.h - (ascent + descent)) / 2;
    // SAFETY: the saved DC state (clip, font, colors) is restored at the end.
    unsafe {
        let saved = SaveDC(hdc);
        IntersectClipRect(hdc, r.x, r.y, r.right(), r.bottom());
        SetBkMode(hdc, TRANSPARENT);
        let mut cx = x;
        for (run, l) in runs.iter().zip(&laid) {
            let y = top + ascent - l.asc;
            if let Some(bg) = run.style.bg {
                fill(hdc, Rect::new(cx, y, l.w, l.h), bg);
            }
            SelectObject(hdc, l.font);
            SetTextColor(hdc, colorref(run.style.fg.unwrap_or(default_fg)));
            let _ = TextOutW(hdc, cx, y, &l.wide);
            cx = cx.saturating_add(l.w);
        }
        let _ = RestoreDC(hdc, saved);
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
    fn base() -> Font {
        Font {
            family: "Segoe UI".into(),
            size_pt: 10.0,
        }
    }
    fn style() -> Style {
        Style {
            scale: 1.0,
            ..Style::default()
        }
    }
    #[test]
    fn run_font_defaults_and_overrides() {
        assert_eq!(
            run_font_spec(&base(), &style()),
            ("Segoe UI".into(), 10.0, 400)
        );
        let s = Style {
            bold: true,
            mono: true,
            scale: 1.2,
            ..style()
        };
        assert_eq!(run_font_spec(&base(), &s), ("Consolas".into(), 12.0, 700));
        let s = Style {
            weight: Some(300),
            family: Some("Arial".into()),
            size_pt: Some(14.0),
            ..style()
        };
        assert_eq!(run_font_spec(&base(), &s), ("Arial".into(), 14.0, 300));
    }
    #[test]
    fn run_font_ignores_bad_sizes_and_clamps() {
        for bad in [f32::NAN, f32::INFINITY, -2.0, 0.0] {
            let s = Style {
                scale: bad,
                size_pt: Some(bad),
                ..style()
            };
            assert_eq!(run_font_spec(&base(), &s).1, 10.0);
        }
        let s = Style {
            size_pt: Some(1000.0),
            ..style()
        };
        assert_eq!(run_font_spec(&base(), &s).1, 200.0);
        let s = Style {
            size_pt: Some(0.2),
            ..style()
        };
        assert_eq!(run_font_spec(&base(), &s).1, 1.0);
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
