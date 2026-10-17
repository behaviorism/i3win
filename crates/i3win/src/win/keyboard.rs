//! Low-level keyboard hook plus a pure matcher that decides which keys fire bindings.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use i3win_core::keys::{scancode_combo, KeyCombo, Modifiers};
use windows::core::Result;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
};

use crate::mainloop::{main_hwnd, WM_APP_KEY, WM_APP_MODS};

const HC_ACTION: i32 = 0;
const LLKHF_EXTENDED: u32 = 0x01;
const LLKHF_INJECTED: u32 = 0x10;
/// `dwExtraInfo` tag on keys i3win injects itself. Only these are ignored; other injected
/// input (AutoHotkey remaps, on-screen keyboards, remote tools) fires bindings like real keys.
pub const OWN_INPUT_TAG: usize = 0x6933_7769;

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
    /// Swallowed press of a release-only binding under Alt/Win: the main loop must send the
    /// menu-masking dummy key now, since the modifier may be released before the key.
    Mask,
    Fire {
        combo: KeyCombo,
        mask_menu: bool,
        release: bool,
    },
}

pub struct KeyMatcher {
    down_mods: HashSet<u16>,
    /// combo -> (has a press binding, has a release binding)
    combos: HashMap<KeyCombo, (bool, bool)>,
    /// Swallowed keys -> the release binding to fire on their key-up, with its mask_menu.
    swallowed: HashMap<u16, Option<(KeyCombo, bool)>>,
    /// Modifier whose held state is reported by `watch_flip` (Hide-mode bars).
    watch: Option<Modifiers>,
    watch_held: bool,
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
            combos: HashMap::new(),
            swallowed: HashMap::new(),
            watch: None,
            watch_held: false,
        }
    }

    pub fn set_combos(&mut self, combos: &[(KeyCombo, bool)]) {
        self.combos.clear();
        for (combo, release) in combos {
            let e = self.combos.entry(*combo).or_default();
            if *release {
                e.1 = true;
            } else {
                e.0 = true;
            }
        }
    }

    /// Sets the modifier to watch (`None` disables); the current state is the
    /// baseline, which is returned so the caller can re-sync its held flag.
    pub fn set_watch(&mut self, watch: Option<Modifiers>) -> bool {
        self.watch = watch;
        self.watch_held = watch.is_some_and(|m| self.mods().contains(m));
        self.watch_held
    }

    /// Re-derives the watched modifier's held state from the tracked keys and
    /// returns it when it flipped (e.g. after `resync` dropped a missed key-up).
    pub fn resync_watch(&mut self) -> Option<bool> {
        let watch = self.watch?;
        let held = self.mods().contains(watch);
        (held != std::mem::replace(&mut self.watch_held, held)).then_some(held)
    }

    /// Applies a modifier key event and returns the new held state of the watched
    /// modifier when it flipped (`None` for repeats, other keys or no watch).
    pub fn watch_flip(&mut self, vk: u16, down: bool) -> Option<bool> {
        self.watch?;
        modifier_of(vk)?;
        if down {
            self.down_mods.insert(vk);
        } else {
            self.down_mods.remove(&vk);
        }
        self.resync_watch()
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

    pub fn on_key(
        &mut self,
        vk: u16,
        down: bool,
        own: bool,
        scancode: u8,
        extended: bool,
    ) -> KeyAction {
        if own {
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
            let mask_menu = mods.contains(Modifiers::ALT) || mods.contains(Modifiers::WIN);
            let by_vk = KeyCombo { mods, vk };
            let by_scan = scancode_combo(mods, scancode, extended);
            let mut press = None;
            let mut release = None;
            for combo in [by_vk, by_scan] {
                if let Some(&(p, r)) = self.combos.get(&combo) {
                    if p && press.is_none() {
                        press = Some(combo);
                    }
                    if r && release.is_none() {
                        release = Some((combo, mask_menu));
                    }
                }
            }
            if press.is_some() || release.is_some() {
                self.swallowed.insert(vk, release);
                return match press {
                    Some(combo) => KeyAction::Fire {
                        combo,
                        mask_menu,
                        release: false,
                    },
                    None if mask_menu => KeyAction::Mask,
                    None => KeyAction::Swallow,
                };
            }
            // A missed key-up must not leave this vk swallowing a later key-up.
            self.swallowed.remove(&vk);
            KeyAction::Pass
        } else {
            match self.swallowed.remove(&vk) {
                Some(Some((combo, mask_menu))) => KeyAction::Fire {
                    combo,
                    mask_menu,
                    release: true,
                },
                Some(None) => KeyAction::Swallow,
                None => KeyAction::Pass,
            }
        }
    }

    /// Drop modifiers the OS no longer reports as held (missed key-ups, e.g. Win+L).
    pub fn resync(&mut self, is_down: impl Fn(u16) -> bool) {
        self.down_mods.retain(|vk| is_down(*vk));
    }
}

thread_local! {
    static MATCHER: RefCell<KeyMatcher> = RefCell::new(KeyMatcher::new());
    static QUEUE: RefCell<Vec<(Option<KeyCombo>, bool, bool)>> = const { RefCell::new(Vec::new()) };
    static HOOK: Cell<Option<HHOOK>> = const { Cell::new(None) };
}

/// Tells the main window the watched modifier's held state flipped.
fn post_mods(held: bool) {
    let main = main_hwnd();
    if main != 0 {
        // SAFETY: plain Win32 call; posting never blocks the hook.
        unsafe {
            let _ = PostMessageW(
                HWND(main as *mut _),
                WM_APP_MODS,
                WPARAM(held as usize),
                LPARAM(0),
            );
        }
    }
}

unsafe extern "system" fn ll_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION && lparam.0 != 0 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let vk = info.vkCode as u16;
        let own = info.flags.0 & LLKHF_INJECTED != 0 && info.dwExtraInfo == OWN_INPUT_TAG;
        let down = wparam.0 as u32 == WM_KEYDOWN || wparam.0 as u32 == WM_SYSKEYDOWN;
        let flip = if own {
            None
        } else {
            MATCHER.with(|m| {
                m.try_borrow_mut()
                    .ok()
                    .and_then(|mut m| m.watch_flip(vk, down))
            })
        };
        if let Some(held) = flip {
            post_mods(held);
        }
        let (action, resynced) = MATCHER.with(|m| match m.try_borrow_mut() {
            Ok(mut m) => {
                let mut resynced = None;
                if down && !own && modifier_of(vk).is_none() {
                    m.resync(|v| GetAsyncKeyState(v as i32) < 0);
                    // A dropped stuck modifier must also release a revealed Hide bar.
                    resynced = m.resync_watch();
                }
                let action = m.on_key(
                    vk,
                    down,
                    own,
                    info.scanCode as u8,
                    info.flags.0 & LLKHF_EXTENDED != 0,
                );
                (action, resynced)
            }
            Err(_) => (KeyAction::Pass, None),
        });
        if let Some(held) = resynced {
            post_mods(held);
        }
        match action {
            KeyAction::Fire { .. } | KeyAction::Mask => {
                let item = match action {
                    KeyAction::Fire {
                        combo,
                        mask_menu,
                        release,
                    } => (Some(combo), mask_menu, release),
                    _ => (None, true, false),
                };
                let queued = QUEUE.with(|q| match q.try_borrow_mut() {
                    Ok(mut q) => {
                        q.push(item);
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

pub fn set_combos(combos: &[(KeyCombo, bool)]) {
    MATCHER.with(|m| {
        if let Ok(mut m) = m.try_borrow_mut() {
            m.set_combos(combos);
        }
    });
}

/// Sets the modifier watched for Hide-mode bars and returns its current held
/// state (after dropping modifiers the OS no longer reports), the new baseline.
pub fn set_modifier_watch(mods: Option<Modifiers>) -> bool {
    MATCHER.with(|m| match m.try_borrow_mut() {
        Ok(mut m) => {
            // SAFETY: plain Win32 call.
            m.resync(|v| unsafe { GetAsyncKeyState(v as i32) } < 0);
            m.set_watch(mods)
        }
        Err(_) => false,
    })
}

/// Modifiers currently held per the hook's tracking (empty if the matcher is busy).
/// Called from the mouse hook: no allocation, no Win32 calls.
pub fn held_mods() -> Modifiers {
    MATCHER.with(|m| m.try_borrow().map(|m| m.mods()).unwrap_or_default())
}

pub fn drain() -> Vec<(Option<KeyCombo>, bool, bool)> {
    QUEUE.with(|q| {
        q.try_borrow_mut()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn resync_releases_watched_modifier() {
        let mut k = KeyMatcher::new();
        k.set_watch(Some(Modifiers::WIN));
        assert_eq!(k.watch_flip(0x5B, true), Some(true));
        // Win's key-up was missed (Win+L); the OS no longer reports it held.
        k.resync(|_| false);
        assert_eq!(k.resync_watch(), Some(false));
        assert_eq!(k.resync_watch(), None);
    }
    #[test]
    fn resync_watch_silent_without_flip_or_watch() {
        let mut k = KeyMatcher::new();
        assert_eq!(k.resync_watch(), None);
        k.set_watch(Some(Modifiers::WIN));
        k.watch_flip(0x5B, true);
        k.resync(|_| true);
        assert_eq!(k.resync_watch(), None);
    }
    #[test]
    fn set_watch_reports_baseline() {
        let mut k = KeyMatcher::new();
        assert!(!k.set_watch(Some(Modifiers::WIN)));
        k.on_key(0x5B, true, false, 0, false);
        assert!(k.set_watch(Some(Modifiers::WIN)));
        assert!(!k.set_watch(Some(Modifiers::ALT)));
        assert!(!k.set_watch(None));
    }
    #[test]
    fn modifier_watch_disabled_without_hide_bars() {
        let mut k = KeyMatcher::new();
        k.set_watch(None);
        assert_eq!(k.watch_flip(0x5B, true), None);
        k.set_watch(Some(Modifiers::WIN));
        assert_eq!(k.watch_flip(0x5B, true), Some(true));
        assert_eq!(k.watch_flip(0x5B, true), None);
        assert_eq!(k.watch_flip(0x5B, false), Some(false));
    }
    use super::*;
    use i3win_core::keys::parse_combo;
    fn m(binds: &[&str]) -> KeyMatcher {
        let mut k = KeyMatcher::new();
        k.set_combos(
            &binds
                .iter()
                .map(|b| (parse_combo(b).unwrap(), false))
                .collect::<Vec<_>>(),
        );
        k
    }
    #[test]
    fn fires_and_swallows_up() {
        let mut k = m(&["Mod1+Return"]);
        assert_eq!(k.on_key(0xA4, true, false, 0, false), KeyAction::Pass);
        assert_eq!(
            k.on_key(0x0D, true, false, 0, false),
            KeyAction::Fire {
                combo: parse_combo("Mod1+Return").unwrap(),
                mask_menu: true,
                release: false
            }
        );
        assert_eq!(k.on_key(0x0D, false, false, 0, false), KeyAction::Swallow);
        assert_eq!(k.on_key(0xA4, false, false, 0, false), KeyAction::Pass);
        assert_eq!(k.on_key(0x0D, true, false, 0, false), KeyAction::Pass);
    }
    #[test]
    fn missed_key_up_is_forgotten_on_next_plain_key_down() {
        let mut k = m(&["Mod1+Return"]);
        k.on_key(0xA4, true, false, 0, false);
        assert!(matches!(
            k.on_key(0x0D, true, false, 0, false),
            KeyAction::Fire { .. }
        ));
        // key-up of Return and Alt were missed; Alt is released later.
        k.on_key(0xA4, false, false, 0, false);
        assert_eq!(k.on_key(0x0D, true, false, 0, false), KeyAction::Pass);
        assert_eq!(k.on_key(0x0D, false, false, 0, false), KeyAction::Pass);
    }
    #[test]
    fn exact_modifier_match() {
        let mut k = m(&["Mod1+q"]);
        k.on_key(0xA4, true, false, 0, false);
        k.on_key(0xA0, true, false, 0, false);
        assert_eq!(k.on_key(0x51, true, false, 0, false), KeyAction::Pass);
    }
    #[test]
    fn bare_keys_dont_mask_menu() {
        let mut k = m(&["Escape"]);
        assert_eq!(
            k.on_key(0x1B, true, false, 0, false),
            KeyAction::Fire {
                combo: parse_combo("Escape").unwrap(),
                mask_menu: false,
                release: false
            }
        );
    }
    #[test]
    fn own_injected_passes() {
        let mut k = m(&["Escape"]);
        assert_eq!(k.on_key(0x1B, true, true, 0, false), KeyAction::Pass);
    }
    #[test]
    fn resync_clears_stuck_modifiers() {
        let mut k = m(&["q"]);
        k.on_key(0x5B, true, false, 0, false);
        k.resync(|_| false);
        assert_eq!(
            k.on_key(0x51, true, false, 0, false),
            KeyAction::Fire {
                combo: parse_combo("q").unwrap(),
                mask_menu: false,
                release: false
            }
        );
    }
    #[test]
    fn matcher_is_pure_and_fast() {
        let mut k = m(&["Mod1+Return", "Mod1+j", "Mod1+k"]);
        let t = std::time::Instant::now();
        for i in 0..10_000u32 {
            k.on_key(0x41 + (i % 26) as u16, i % 2 == 0, false, 0, false);
        }
        assert!(t.elapsed() < std::time::Duration::from_millis(50));
    }
    #[test]
    fn release_combo_fires_on_key_up() {
        let mut k = KeyMatcher::new();
        k.set_combos(&[(parse_combo("Mod1+x").unwrap(), true)]);
        k.on_key(0xA4, true, false, 0, false);
        assert_eq!(k.on_key(0x58, true, false, 0x2D, false), KeyAction::Mask);
        assert!(matches!(
            k.on_key(0x58, false, false, 0x2D, false),
            KeyAction::Fire { release: true, .. }
        ));
        assert_eq!(k.on_key(0x58, false, false, 0x2D, false), KeyAction::Pass);
    }
    #[test]
    fn scancode_combo_matches() {
        let mut k = KeyMatcher::new();
        k.set_combos(&[(scancode_combo(Modifiers::default(), 0x1E, false), false)]);
        assert!(matches!(
            k.on_key(0x41, true, false, 0x1E, false),
            KeyAction::Fire { release: false, .. }
        ));
        assert_eq!(k.on_key(0x41, true, false, 0x1F, false), KeyAction::Pass);
        assert_eq!(k.on_key(0x41, true, false, 0x1E, true), KeyAction::Pass);
    }
    #[test]
    fn release_only_bare_key_swallows_without_mask() {
        let mut k = KeyMatcher::new();
        k.set_combos(&[(parse_combo("x").unwrap(), true)]);
        assert_eq!(k.on_key(0x58, true, false, 0x2D, false), KeyAction::Swallow);
        assert!(matches!(
            k.on_key(0x58, false, false, 0x2D, false),
            KeyAction::Fire { release: true, .. }
        ));
    }
    #[test]
    fn press_and_release_bindings_both_fire() {
        let mut k = KeyMatcher::new();
        let c = parse_combo("Mod1+x").unwrap();
        k.set_combos(&[(c, false), (c, true)]);
        k.on_key(0xA4, true, false, 0, false);
        assert!(matches!(
            k.on_key(0x58, true, false, 0x2D, false),
            KeyAction::Fire { release: false, .. }
        ));
        assert!(matches!(
            k.on_key(0x58, false, false, 0x2D, false),
            KeyAction::Fire { release: true, .. }
        ));
    }
}
