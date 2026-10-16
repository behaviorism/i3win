//! Low-level keyboard hook plus a pure matcher that decides which keys fire bindings.
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use i3win_core::keys::{KeyCombo, Modifiers};
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::mainloop::{main_hwnd, WM_APP_KEY};

const HC_ACTION: i32 = 0;
const LLKHF_INJECTED: u32 = 0x10;

const MODIFIER_VKS: [(u16, Modifiers); 11] = [
    (0x12, Modifiers::ALT),
    (0xA4, Modifiers::ALT),
    (0xA5, Modifiers::ALT),
    (0x10, Modifiers::SHIFT),
    (0xA0, Modifiers::SHIFT),
    (0xA1, Modifiers::SHIFT),
    (0x11, Modifiers::CTRL),
    (0xA2, Modifiers::CTRL),
    (0xA3, Modifiers::CTRL),
    (0x5B, Modifiers::WIN),
    (0x5C, Modifiers::WIN),
];

fn modifier_of(vk: u16) -> Option<Modifiers> {
    MODIFIER_VKS.iter().find(|(v, _)| *v == vk).map(|(_, m)| *m)
}

#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    Pass,
    Swallow,
    Fire { combo: KeyCombo, mask_menu: bool },
}

pub struct KeyMatcher {
    down_mods: HashSet<u16>,
    combos: HashSet<KeyCombo>,
    swallowed: HashSet<u16>,
}

impl Default for KeyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyMatcher {
    pub fn new() -> Self {
        KeyMatcher {
            down_mods: HashSet::new(),
            combos: HashSet::new(),
            swallowed: HashSet::new(),
        }
    }

    pub fn set_combos(&mut self, combos: &[KeyCombo]) {
        self.combos = combos.iter().copied().collect();
    }

    pub fn mods(&self) -> Modifiers {
        let mut m = Modifiers::default();
        for vk in &self.down_mods {
            if let Some(x) = modifier_of(*vk) {
                m = m | x;
            }
        }
        m
    }

    pub fn on_key(&mut self, vk: u16, down: bool, injected: bool) -> KeyAction {
        if injected {
            return KeyAction::Pass;
        }
        if modifier_of(vk).is_some() {
            if down {
                self.down_mods.insert(vk);
            } else {
                self.down_mods.remove(&vk);
            }
            return KeyAction::Pass;
        }
        if down {
            let mods = self.mods();
            let combo = KeyCombo { mods, vk };
            if self.combos.contains(&combo) {
                self.swallowed.insert(vk);
                let mask_menu = mods.contains(Modifiers::ALT) || mods.contains(Modifiers::WIN);
                return KeyAction::Fire { combo, mask_menu };
            }
            KeyAction::Pass
        } else if self.swallowed.remove(&vk) {
            KeyAction::Swallow
        } else {
            KeyAction::Pass
        }
    }

    /// Drop modifiers the OS no longer reports as held (missed key-ups, e.g. Win+L).
    pub fn resync(&mut self, is_down: impl Fn(u16) -> bool) {
        self.down_mods.retain(|vk| is_down(*vk));
    }
}

thread_local! {
    static MATCHER: RefCell<KeyMatcher> = RefCell::new(KeyMatcher::new());
    static QUEUE: RefCell<Vec<(KeyCombo, bool)>> = const { RefCell::new(Vec::new()) };
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
}

unsafe extern "system" fn ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION && lparam.0 != 0 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = info.vkCode as u16;
        let injected = info.flags.0 & LLKHF_INJECTED != 0;
        let down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
        let action = MATCHER.with(|m| match m.try_borrow_mut() {
            Ok(mut m) => {
                if down && !injected && modifier_of(vk).is_none() {
                    m.resync(|v| GetAsyncKeyState(v as i32) < 0);
                }
                m.on_key(vk, down, injected)
            }
            Err(_) => KeyAction::Pass,
        });
        match action {
            KeyAction::Fire { combo, mask_menu } => {
                let queued = QUEUE.with(|q| match q.try_borrow_mut() {
                    Ok(mut q) => {
                        q.push((combo, mask_menu));
                        true
                    }
                    Err(_) => false,
                });
                if queued {
                    let main = main_hwnd();
                    if main != 0 {
                        let _ =
                            PostMessageW(HWND(main as *mut _), WM_APP_KEY, WPARAM(0), LPARAM(0));
                    }
                    return LRESULT(1);
                }
            }
            KeyAction::Swallow => return LRESULT(1),
            KeyAction::Pass => {}
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

pub fn install() -> Result<()> {
    if HOOK.with(|h| h.get().is_some()) {
        return Ok(());
    }
    let hmod = unsafe { GetModuleHandleW(None)? };
    let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(ll_proc), hmod, 0)? };
    HOOK.with(|c| c.set(Some(h)));
    Ok(())
}

pub fn uninstall() {
    if let Some(h) = HOOK.with(|c| c.take()) {
        unsafe {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

pub fn set_combos(combos: &[KeyCombo]) {
    MATCHER.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.set_combos(combos);
        }
    });
}

pub fn drain() -> Vec<(KeyCombo, bool)> {
    QUEUE.with(|q| {
        q.try_borrow_mut()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use i3win_core::keys::parse_combo;
    fn m(binds: &[&str]) -> KeyMatcher {
        let mut k = KeyMatcher::new();
        k.set_combos(
            &binds
                .iter()
                .map(|b| parse_combo(b).unwrap())
                .collect::<Vec<_>>(),
        );
        k
    }
    #[test]
    fn fires_and_swallows_up() {
        let mut k = m(&["Mod1+Return"]);
        assert_eq!(k.on_key(0xA4, true, false), KeyAction::Pass);
        assert_eq!(
            k.on_key(0x0D, true, false),
            KeyAction::Fire {
                combo: parse_combo("Mod1+Return").unwrap(),
                mask_menu: true
            }
        );
        assert_eq!(k.on_key(0x0D, false, false), KeyAction::Swallow);
        assert_eq!(k.on_key(0xA4, false, false), KeyAction::Pass);
        assert_eq!(k.on_key(0x0D, true, false), KeyAction::Pass);
    }
    #[test]
    fn exact_modifier_match() {
        let mut k = m(&["Mod1+q"]);
        k.on_key(0xA4, true, false);
        k.on_key(0xA0, true, false);
        assert_eq!(k.on_key(0x51, true, false), KeyAction::Pass);
    }
    #[test]
    fn bare_keys_dont_mask_menu() {
        let mut k = m(&["Escape"]);
        assert_eq!(
            k.on_key(0x1B, true, false),
            KeyAction::Fire {
                combo: parse_combo("Escape").unwrap(),
                mask_menu: false
            }
        );
    }
    #[test]
    fn injected_passes() {
        let mut k = m(&["Escape"]);
        assert_eq!(k.on_key(0x1B, true, true), KeyAction::Pass);
    }
    #[test]
    fn resync_clears_stuck_modifiers() {
        let mut k = m(&["q"]);
        k.on_key(0x5B, true, false);
        k.resync(|_| false);
        assert_eq!(
            k.on_key(0x51, true, false),
            KeyAction::Fire {
                combo: parse_combo("q").unwrap(),
                mask_menu: false
            }
        );
    }
    #[test]
    fn matcher_is_pure_and_fast() {
        let mut k = m(&["Mod1+Return", "Mod1+j", "Mod1+k"]);
        let t = std::time::Instant::now();
        for i in 0..10_000u32 {
            k.on_key(0x41 + (i % 26) as u16, i % 2 == 0, false);
        }
        assert!(t.elapsed() < std::time::Duration::from_millis(50));
    }
}
