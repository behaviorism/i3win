use std::ops::BitOr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub const ALT: Modifiers = Modifiers(1);
    pub const SHIFT: Modifiers = Modifiers(2);
    pub const CTRL: Modifiers = Modifiers(4);
    pub const WIN: Modifiers = Modifiers(8);

    pub fn contains(self, other: Modifiers) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, rhs: Modifiers) -> Modifiers {
        Modifiers(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub mods: Modifiers,
    pub vk: u16,
}

/// `KeyCombo.vk` with this bit set is a scan code: bits 0-7 the code, bit 8 (0x100) extended.
pub const SCANCODE_FLAG: u16 = 0x8000;

pub fn scancode_combo(mods: Modifiers, scancode: u8, extended: bool) -> KeyCombo {
    KeyCombo {
        mods,
        vk: SCANCODE_FLAG | scancode as u16 | if extended { 0x100 } else { 0 },
    }
}

/// Map an i3/X11 keysym name to a Windows virtual-key code.
pub fn keysym_to_vk(name: &str) -> Option<u16> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if c.is_ascii_alphabetic() {
            return Some(c.to_ascii_uppercase() as u16);
        }
        if c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    if let Some(n) = name.strip_prefix('F').and_then(|d| d.parse::<u16>().ok()) {
        if (1..=24).contains(&n) && !name[1..].starts_with('+') {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = name.strip_prefix("KP_").and_then(|d| d.parse::<u16>().ok()) {
        if n <= 9 && name.len() == 4 {
            return Some(0x60 + n);
        }
    }
    Some(match name {
        "Return" => 0x0D,
        "space" => 0x20,
        "Escape" => 0x1B,
        "Tab" => 0x09,
        "BackSpace" => 0x08,
        "Delete" => 0x2E,
        "Insert" => 0x2D,
        "Home" => 0x24,
        "End" => 0x23,
        "Prior" | "Page_Up" => 0x21,
        "Next" | "Page_Down" => 0x22,
        "Left" => 0x25,
        "Up" => 0x26,
        "Right" => 0x27,
        "Down" => 0x28,
        "minus" => 0xBD,
        "equal" | "plus" => 0xBB,
        "comma" => 0xBC,
        "period" => 0xBE,
        "slash" => 0xBF,
        "semicolon" => 0xBA,
        "grave" => 0xC0,
        "bracketleft" => 0xDB,
        "backslash" => 0xDC,
        "bracketright" => 0xDD,
        "apostrophe" => 0xDE,
        "Print" => 0x2C,
        "Pause" => 0x13,
        "XF86AudioMute" => 0xAD,
        "XF86AudioLowerVolume" => 0xAE,
        "XF86AudioRaiseVolume" => 0xAF,
        "XF86AudioNext" => 0xB0,
        "XF86AudioPrev" => 0xB1,
        "XF86AudioPlay" => 0xB3,
        _ => return None,
    })
}

/// Parse a combo such as `Mod1+Shift+Return`. All parts but the last are modifiers.
pub fn parse_combo(s: &str) -> Result<KeyCombo, String> {
    let (mods, key) = split_combo(s)?;
    let vk = keysym_to_vk(key).ok_or_else(|| format!("unknown key '{key}' in '{s}'"))?;
    Ok(KeyCombo { mods, vk })
}

/// Split `Mod1+Shift+KEY` into its modifiers and the trailing key text.
pub fn split_combo(s: &str) -> Result<(Modifiers, &str), String> {
    let parts: Vec<&str> = s.split('+').collect();
    let (key, mod_parts) = parts.split_last().ok_or("empty key combo")?;
    let mut mods = Modifiers::default();
    for m in mod_parts {
        mods = mods
            | match *m {
                "Mod1" => Modifiers::ALT,
                "Mod4" => Modifiers::WIN,
                "Shift" => Modifiers::SHIFT,
                "Control" | "Ctrl" => Modifiers::CTRL,
                other => return Err(format!("unknown modifier '{other}' in '{s}'")),
            };
    }
    Ok((mods, *key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mod_shift_return() {
        let c = parse_combo("Mod1+Shift+Return").unwrap();
        assert_eq!(
            c,
            KeyCombo {
                mods: Modifiers::ALT | Modifiers::SHIFT,
                vk: 0x0D
            }
        );
    }
    #[test]
    fn letters_case_insensitive() {
        assert_eq!(parse_combo("Mod4+Q").unwrap().vk, 0x51);
        assert_eq!(parse_combo("Mod4+q").unwrap().vk, 0x51);
    }
    #[test]
    fn bare_key_has_no_mods() {
        assert_eq!(
            parse_combo("Escape").unwrap(),
            KeyCombo {
                mods: Modifiers(0),
                vk: 0x1B
            }
        );
    }
    #[test]
    fn function_and_symbol_keys() {
        assert_eq!(keysym_to_vk("F12"), Some(0x7B));
        assert_eq!(keysym_to_vk("semicolon"), Some(0xBA));
        assert_eq!(keysym_to_vk("KP_5"), Some(0x65));
    }
    #[test]
    fn rejects_unknown() {
        assert!(parse_combo("Mod2+a").is_err());
        assert!(parse_combo("Mod1+nosuchkey").is_err());
        assert!(parse_combo("").is_err());
    }
    #[test]
    fn ctrl_alias() {
        assert_eq!(
            parse_combo("Ctrl+a").unwrap(),
            parse_combo("Control+a").unwrap()
        );
    }
}
