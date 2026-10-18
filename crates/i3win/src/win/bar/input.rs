//! Bar input: click and wheel handling, routed to status clicks or commands.
use i3win_core::geom::Rect;
use i3win_core::i3bar::{Block, Click};
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use super::window::BarData;
use crate::bar_layout::{
    bar_binding_for, default_command, hit_test, route_bar_button, BarAction, BarItem, ItemKind,
};
use crate::mainloop::{self, MainMsg};

thread_local! {
    /// Wheel delta not yet worth a full notch (touchpads send small deltas).
    pub(super) static WHEEL_ACC: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    /// Same for the horizontal wheel.
    pub(super) static HWHEEL_ACC: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
}

/// Adds `delta` to the accumulator and returns whole notches: positive is wheel
/// up (previous workspace), negative is down. The remainder stays in `acc`.
pub(super) fn wheel_steps(acc: &mut i32, delta: i32) -> i32 {
    const WHEEL_DELTA: i32 = 120;
    *acc += delta;
    let steps = *acc / WHEEL_DELTA;
    *acc -= steps * WHEEL_DELTA;
    steps
}

/// Sends a click on status block `item` (client point `x, y`) to the bar's status process.
fn status_click(hwnd: HWND, data: &BarData, item: &BarItem, button: u8, x: i32, y: i32) {
    let ItemKind::Block(i) = item.kind else {
        return;
    };
    let mut pt = POINT { x, y };
    // SAFETY: plain Win32 call on our own window.
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
    }
    let block = data.blocks.get(i);
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
        click: block_click(
            block,
            item.rect,
            button,
            modifiers,
            (x, y),
            (pt.x, pt.y),
            data.output_origin,
        ),
    });
}

/// The i3bar click event for `button` at bar-client point `client` (screen point
/// `screen`) on a block occupying `rect`; `output_x/y` are relative to the
/// output's work-area origin `output`.
fn block_click(
    block: Option<&Block>,
    rect: Rect,
    button: u8,
    modifiers: Vec<String>,
    client: (i32, i32),
    screen: (i32, i32),
    output: (i32, i32),
) -> Click {
    Click {
        name: block.and_then(|b| b.name.clone()),
        instance: block.and_then(|b| b.instance.clone()),
        button,
        modifiers,
        x: screen.0,
        y: screen.1,
        relative_x: client.0 - rect.x,
        relative_y: client.1 - rect.y,
        output_x: screen.0 - output.0,
        output_y: screen.1 - output.1,
        width: rect.w,
        height: rect.h,
    }
}

/// Handles `count` events of mouse `button` (i3 numbering) at client point
/// `x, y`: a status click, a bar binding, or the default, per `route_bar_button`.
pub(super) fn on_button(
    hwnd: HWND,
    data: &BarData,
    button: u8,
    release: bool,
    x: i32,
    y: i32,
    count: u32,
) {
    let hit = hit_test(&data.layout, x, y);
    let kind = hit.map(|i| &i.kind);
    let binding = bar_binding_for(&data.bindings, button, release);
    let cmd = match route_bar_button(kind, data.wants_clicks, release, binding) {
        BarAction::StatusClick => {
            if let Some(item) = hit {
                for _ in 0..count {
                    status_click(hwnd, data, item, button, x, y);
                }
            }
            return;
        }
        BarAction::Command(c) => c,
        BarAction::Default => match default_command(kind, button) {
            Some(c) => c,
            None => return,
        },
        BarAction::Nothing => return,
    };
    for _ in 0..count {
        mainloop::post(bar_command(cmd.clone()));
    }
}

/// A bar click or wheel runs its command without warping the pointer: the pointer
/// is on the bar the user is using, possibly on another output than the focus.
fn bar_command(cmd: String) -> MainMsg {
    MainMsg::MouseCommand(cmd)
}

/// The point packed in a mouse message's `lParam` (client or screen coordinates).
pub(super) fn lparam_point(lp: LPARAM) -> (i32, i32) {
    (
        (lp.0 & 0xffff) as i16 as i32,
        ((lp.0 >> 16) & 0xffff) as i16 as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bar_commands_never_warp() {
        assert!(matches!(
            bar_command("workspace 2".into()),
            MainMsg::MouseCommand(c) if c == "workspace 2"
        ));
    }
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
    fn block_click_is_relative_to_work_area_origin() {
        let block = Block {
            name: Some("cpu".into()),
            instance: Some("0".into()),
            ..Default::default()
        };
        // Bar on a monitor at x=1920 whose work area starts below a 40 px top taskbar.
        let c = block_click(
            Some(&block),
            Rect::new(900, 0, 60, 20),
            4,
            vec!["Shift".into()],
            (910, 5),
            (2830, 45),
            (1920, 40),
        );
        assert_eq!((c.x, c.y), (2830, 45));
        assert_eq!((c.output_x, c.output_y), (910, 5));
        assert_eq!((c.relative_x, c.relative_y), (10, 5));
        assert_eq!((c.width, c.height, c.button), (60, 20, 4));
        assert_eq!(c.name.as_deref(), Some("cpu"));
        assert_eq!(c.instance.as_deref(), Some("0"));
    }
}
