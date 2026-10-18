//! Bar windows: one topmost, never-activating GDI strip per output.
//!
//! The window procedure never touches `APP`: it paints from the `BarData`
//! stored in `GWLP_USERDATA` and only posts `MainMsg`s on clicks and wheel turns.
mod input;
mod paint;
mod window;

use std::collections::{HashMap, HashSet};

use i3win_core::arrange::{bar_applies, bar_height_of, Arrangement};
use i3win_core::bar_model::workspace_buttons;
use i3win_core::config::{BarMode, BarPosition, HiddenState};
use i3win_core::geom::Rect;
use i3win_core::i3bar::Block;
use i3win_core::tree::{NodeKind, WindowId};
use i3win_core::wm::Wm;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::bar_layout::{hit_test, layout_bar, BarStyle, ItemKind};
use paint::measure_text;
pub use window::BarData;
use window::{create_window, register_class, set_data};

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
    /// (latest status blocks per bar id); `click_bars` are the bar ids whose status
    /// process asked for click events. Dock bars reserve space; Hide bars are
    /// overlays shown while `hidden_state` is show, `modifier_held`, or a
    /// workspace on the output is urgent; Invisible bars get no window.
    pub fn update(
        &mut self,
        wm: &Wm,
        arr: &Arrangement,
        blocks: &HashMap<String, Vec<Block>>,
        click_bars: &HashSet<String>,
        modifier_held: bool,
    ) {
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
                let (bar_mode, hidden_state) = wm.bar_state_of(&bar.id);
                if bar_mode == BarMode::Invisible || !bar_applies(bar, name, *primary) {
                    continue;
                }
                let h = bar_height_of(bar, &wm.config, scale);
                let mut overlay_offset = 0;
                let offset = match bar.position {
                    _ if bar_mode == BarMode::Hide => &mut overlay_offset,
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
                    bindings: bar.bindings.clone(),
                    wants_clicks: click_bars.contains(&bar.id),
                    output_origin: (o.x, o.y),
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
                    let revealed = bar_mode == BarMode::Dock
                        || hidden_state == HiddenState::Show
                        || (modifier_held && bar.modifier.is_some())
                        || buttons.iter().any(|b| b.urgent);
                    let want_shown = revealed && !arr.fullscreen_outputs.contains(name);
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

    /// Whether `id` is one of the bar windows.
    pub fn owns(&self, id: WindowId) -> bool {
        self.windows.values().any(|bw| bw.hwnd.0 as isize == id.0)
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

#[cfg(test)]
mod tests {
    use super::*;
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
