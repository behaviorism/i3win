//! Bar painting: items, borders, separators and markup runs.
use i3win_core::config::Font;
use i3win_core::geom::Rect;
use i3win_core::markup::parse_markup;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC, SelectObject};

use super::window::BarData;
use crate::win::gdi;

/// Width in pixels of `text` in `font` at `scale`, measured on a screen DC.
/// With `markup`, `text` is parsed and measured run by run.
pub(super) fn measure_text(font: &Font, scale: f32, text: &str, markup: bool) -> i32 {
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
            let old = SelectObject(hdc, hfont);
            let w = gdi::measure_wide(hdc, text).unwrap_or(0);
            SelectObject(hdc, old);
            w
        };
        ReleaseDC(None, hdc);
        width
    }
}

pub(super) fn paint(hwnd: HWND, data: &BarData) {
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
