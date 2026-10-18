//! Small GDI helpers shared by the decoration windows.
use std::cell::{Cell, RefCell};
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

/// Cached fonts kept before the cache is emptied (markup can ask for many sizes).
const FONT_CACHE_MAX: usize = 256;

thread_local! {
    /// Created fonts as raw handle values, freed by `free_fonts`.
    static FONTS: RefCell<HashMap<FontKey, isize>> = RefCell::new(HashMap::new());
    /// Nesting depth of `paint_buffered`; cached fonts may be selected into a DC
    /// or held by the painter meanwhile, so the cache is never emptied then.
    static PAINTING: Cell<u32> = const { Cell::new(0) };
}

/// Whether a full cache of `len` fonts is emptied before adding another.
fn should_evict(len: usize, painting: bool) -> bool {
    len >= FONT_CACHE_MAX && !painting
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
        // Outside a paint no cached font is selected into a DC (every user restores
        // the previous font right away), so the handles can be deleted safely.
        if should_evict(map.len(), PAINTING.with(Cell::get) > 0) {
            delete_fonts(&mut map);
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
    FONTS.with(|f| delete_fonts(&mut f.borrow_mut()));
}

fn delete_fonts(map: &mut HashMap<FontKey, isize>) {
    for (_, h) in map.drain() {
        // SAFETY: the handle came from CreateFontW and is no longer in use.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(h as *mut _));
        }
    }
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

/// UTF-16 of `text`, or `None` when empty. GDI text calls must never be handed
/// an empty slice (windows-rs passes a dangling pointer and user32 faults).
pub fn wide(text: &str) -> Option<Vec<u16>> {
    if text.is_empty() {
        None
    } else {
        Some(text.encode_utf16().collect())
    }
}

/// Width of `text` with the font selected into `hdc`; 0 for empty text.
///
/// # Safety
/// `hdc` must be a valid DC.
pub unsafe fn measure_wide(hdc: HDC, text: &str) -> Option<i32> {
    let w = wide(text)?;
    let mut size = SIZE::default();
    GetTextExtentPoint32W(hdc, &w, &mut size)
        .as_bool()
        .then_some(size.cx)
}

/// Draws one line of `text` in `r` with `font`, left aligned and vertically centered.
pub fn draw_text(hdc: HDC, r: Rect, text: &str, c: Color, font: HFONT) {
    let Some(mut wide) = wide(text) else {
        return;
    };
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
    let wide: Vec<u16> = wide(&run.text).unwrap_or_default();
    let mut size = SIZE::default();
    let mut tm = TEXTMETRICW::default();
    // SAFETY: out-pointers are valid locals; the previous font is restored.
    unsafe {
        let old = SelectObject(hdc, font);
        if !wide.is_empty() {
            let _ = GetTextExtentPoint32W(hdc, &wide, &mut size);
        }
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

/// Where to cut runs of `widths` starting at `x` so they end by `right`: `None`
/// when they fit, else `(i, avail)`: draw runs before `i` whole, then as much of
/// run `i` as fits in `avail` px followed by an ellipsis `ell[i]` px wide (the
/// ellipsis measured in run `i`'s font).
fn ellipsis_cut(widths: &[i32], ell: &[i32], x: i32, right: i32) -> Option<(usize, i32)> {
    let total = widths.iter().fold(0i32, |a, w| a.saturating_add(*w));
    if x.saturating_add(total) <= right {
        return None;
    }
    let mut cx = x;
    for (i, (w, e)) in widths.iter().zip(ell).enumerate() {
        if cx.saturating_add(*w).saturating_add(*e) > right {
            return Some((i, right.saturating_sub(*e).saturating_sub(cx).max(0)));
        }
        cx = cx.saturating_add(*w);
    }
    None
}

/// UTF-16 units of `wide` that fit in `max` px with the font selected into `hdc`,
/// never splitting a surrogate pair.
///
/// # Safety
/// `hdc` must be a valid DC.
unsafe fn fit_prefix(hdc: HDC, wide: &[u16], max: i32) -> usize {
    if wide.is_empty() {
        return 0;
    }
    let mut fit = 0i32;
    let mut size = SIZE::default();
    let ok = GetTextExtentExPointW(
        hdc,
        PCWSTR(wide.as_ptr()),
        wide.len() as i32,
        max,
        Some(&mut fit),
        None,
        &mut size,
    )
    .as_bool();
    let mut n = if ok {
        (fit.max(0) as usize).min(wide.len())
    } else {
        0
    };
    if n > 0 && (0xD800..0xDC00).contains(&wide[n - 1]) {
        n -= 1;
    }
    n
}

/// Width of `wide` with the font selected into `hdc`.
///
/// # Safety
/// `hdc` must be a valid DC.
unsafe fn text_width(hdc: HDC, wide: &[u16]) -> i32 {
    if wide.is_empty() {
        return 0;
    }
    let mut size = SIZE::default();
    let _ = GetTextExtentPoint32W(hdc, wide, &mut size);
    size.cx.max(0)
}

/// Draws `runs` starting at `x`, vertically centered in `r` (runs share a
/// baseline) and clipped to `r`; runs too wide for `r` end in an ellipsis.
/// A run's `bg` fills its text extent first.
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
    const ELLIPSIS: [u16; 1] = [0x2026];
    let widths: Vec<i32> = laid.iter().map(|l| l.w).collect();
    // SAFETY: the saved DC state (clip, font, colors) is restored at the end.
    unsafe {
        let saved = SaveDC(hdc);
        IntersectClipRect(hdc, r.x, r.y, r.right(), r.bottom());
        SetBkMode(hdc, TRANSPARENT);
        let total = widths.iter().fold(0i32, |a, w| a.saturating_add(*w));
        let cut = if x.saturating_add(total) > r.right() {
            let ell: Vec<i32> = laid
                .iter()
                .map(|l| {
                    SelectObject(hdc, l.font);
                    text_width(hdc, &ELLIPSIS)
                })
                .collect();
            ellipsis_cut(&widths, &ell, x, r.right())
        } else {
            None
        };
        let mut cx = x;
        for (i, (run, l)) in runs.iter().zip(&laid).enumerate() {
            let y = top + ascent - l.asc;
            SelectObject(hdc, l.font);
            SetTextColor(hdc, colorref(run.style.fg.unwrap_or(default_fg)));
            if let Some((_, avail)) = cut.filter(|(c, _)| *c == i) {
                let head = &l.wide[..fit_prefix(hdc, &l.wide, avail)];
                let hw = text_width(hdc, head);
                let ew = text_width(hdc, &ELLIPSIS);
                if let Some(bg) = run.style.bg {
                    fill(hdc, Rect::new(cx, y, hw.saturating_add(ew), l.h), bg);
                }
                if !head.is_empty() {
                    let _ = TextOutW(hdc, cx, y, head);
                }
                let _ = TextOutW(hdc, cx.saturating_add(hw), y, &ELLIPSIS);
                break;
            }
            if let Some(bg) = run.style.bg {
                fill(hdc, Rect::new(cx, y, l.w, l.h), bg);
            }
            if !l.wide.is_empty() {
                let _ = TextOutW(hdc, cx, y, &l.wide);
            }
            cx = cx.saturating_add(l.w);
        }
        let _ = RestoreDC(hdc, saved);
    }
}

/// Double-buffered paint of `hwnd`'s client area; `f` draws into the memory DC.
pub fn paint_buffered(hwnd: HWND, f: impl FnOnce(HDC, Rect)) {
    /// Marks a paint in progress for the font cache, even if `f` unwinds.
    struct PaintGuard;
    impl Drop for PaintGuard {
        fn drop(&mut self) {
            PAINTING.with(|p| p.set(p.get().saturating_sub(1)));
        }
    }
    PAINTING.with(|p| p.set(p.get() + 1));
    let _guard = PaintGuard;
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
    fn wide_is_none_for_empty_text() {
        assert_eq!(wide(""), None);
        assert_eq!(wide("a\u{1F600}"), Some(vec![0x61, 0xD83D, 0xDE00]));
    }
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

    #[test]
    fn ellipsis_cut_points() {
        // Fits exactly: no cut.
        assert_eq!(ellipsis_cut(&[10, 20], &[5, 5], 0, 30), None);
        // Overflows in the second run: it keeps 29 - 5 - 10 = 14 px.
        assert_eq!(ellipsis_cut(&[10, 20], &[5, 5], 0, 29), Some((1, 14)));
        // The first run would leave no room for its ellipsis: cut inside it.
        assert_eq!(ellipsis_cut(&[10, 20], &[5, 5], 0, 12), Some((0, 7)));
        // The start offset counts; `avail` never goes negative.
        assert_eq!(ellipsis_cut(&[10], &[8], 25, 30), Some((0, 0)));
        assert_eq!(ellipsis_cut(&[], &[], 0, 0), None);
    }
    #[test]
    fn font_cache_evicts_only_outside_paint() {
        assert!(!should_evict(FONT_CACHE_MAX - 1, false));
        assert!(should_evict(FONT_CACHE_MAX, false));
        assert!(!should_evict(FONT_CACHE_MAX + 10, true));
    }
    #[test]
    fn font_cache_stays_bounded() {
        for i in 0..(FONT_CACHE_MAX + 50) {
            let _ = font("Segoe UI", 4.0 + i as f32 * 0.1, 1.0);
        }
        let len = FONTS.with(|f| f.borrow().len());
        assert!(len > 0 && len <= FONT_CACHE_MAX, "{len}");
        free_fonts();
    }
}
