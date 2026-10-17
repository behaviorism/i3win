//! X11 keycode (as used by `bindcode`) to PC set-1 scan code.

/// Returns `(scancode, extended)`. X11 keycode = evdev code + 8, and evdev codes 1-88 equal
/// the set-1 scan codes of non-extended keys.
pub fn x11_keycode_to_scancode(code: u16) -> Option<(u8, bool)> {
    match code {
        9..=96 => Some(((code - 8) as u8, false)),
        127 => Some((0x45, false)), // Pause: no extended flag in KBDLLHOOKSTRUCT
        104 => Some((0x1C, true)),  // KP_Enter
        105 => Some((0x1D, true)),  // Control_R
        106 => Some((0x35, true)),  // KP_Divide
        107 => Some((0x37, true)),  // Print
        108 => Some((0x38, true)),  // Alt_R
        110 => Some((0x47, true)),  // Home
        111 => Some((0x48, true)),  // Up
        112 => Some((0x49, true)),  // Prior
        113 => Some((0x4B, true)),  // Left
        114 => Some((0x4D, true)),  // Right
        115 => Some((0x4F, true)),  // End
        116 => Some((0x50, true)),  // Down
        117 => Some((0x51, true)),  // Next
        118 => Some((0x52, true)),  // Insert
        119 => Some((0x53, true)),  // Delete
        133 => Some((0x5B, true)),  // Super_L
        134 => Some((0x5C, true)),  // Super_R
        135 => Some((0x5D, true)),  // Menu
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keycode_table() {
        assert_eq!(x11_keycode_to_scancode(38), Some((0x1E, false))); // a
        assert_eq!(x11_keycode_to_scancode(36), Some((0x1C, false))); // Return
        assert_eq!(x11_keycode_to_scancode(113), Some((0x4B, true))); // Left
        assert_eq!(x11_keycode_to_scancode(105), Some((0x1D, true))); // Control_R
        assert_eq!(x11_keycode_to_scancode(95), Some((0x57, false))); // F11
        assert_eq!(x11_keycode_to_scancode(127), Some((0x45, false))); // Pause
        assert_eq!(x11_keycode_to_scancode(255), None);
    }
}
