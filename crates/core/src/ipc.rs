//! i3 IPC wire framing: `"i3-ipc"`, payload length (u32 LE), type (u32 LE), payload.

pub const MAGIC: &[u8; 6] = b"i3-ipc";
pub const HEADER_LEN: usize = 14;
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

pub const RUN_COMMAND: u32 = 0;
pub const GET_WORKSPACES: u32 = 1;
pub const SUBSCRIBE: u32 = 2;
pub const GET_OUTPUTS: u32 = 3;
pub const GET_TREE: u32 = 4;
pub const GET_MARKS: u32 = 5;
pub const GET_BAR_CONFIG: u32 = 6;
pub const GET_VERSION: u32 = 7;
pub const GET_BINDING_MODES: u32 = 8;
pub const GET_CONFIG: u32 = 9;
pub const SEND_TICK: u32 = 10;

pub const EVENT_BIT: u32 = 0x8000_0000;
pub const EV_WORKSPACE: u32 = 0;
pub const EV_OUTPUT: u32 = 1;
pub const EV_MODE: u32 = 2;
pub const EV_WINDOW: u32 = 3;
pub const EV_BARCONFIG_UPDATE: u32 = 4;
pub const EV_BINDING: u32 = 5;
pub const EV_SHUTDOWN: u32 = 6;
pub const EV_TICK: u32 = 7;

/// Builds a frame. A payload longer than `u32::MAX` bytes gets a saturated
/// length field (never panics); real payloads are far below `MAX_PAYLOAD`.
pub fn encode(msg_type: u32, payload: &str) -> Vec<u8> {
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&msg_type.to_le_bytes());
    out.extend_from_slice(payload.as_bytes());
    out
}

/// Parses a 14-byte header into `(payload_len, msg_type)`.
pub fn decode_header(h: &[u8]) -> Result<(u32, u32), String> {
    if h.len() != HEADER_LEN {
        return Err(format!("bad header length {}", h.len()));
    }
    if &h[0..6] != MAGIC {
        return Err("bad magic".to_string());
    }
    let len = u32::from_le_bytes([h[6], h[7], h[8], h[9]]);
    let ty = u32::from_le_bytes([h[10], h[11], h[12], h[13]]);
    if len > MAX_PAYLOAD {
        return Err(format!("payload too large: {len}"));
    }
    Ok((len, ty))
}

/// Request type for an `i3-msg -t` name.
pub fn type_from_name(name: &str) -> Option<u32> {
    Some(match name {
        "command" => RUN_COMMAND,
        "get_workspaces" => GET_WORKSPACES,
        "subscribe" => SUBSCRIBE,
        "get_outputs" => GET_OUTPUTS,
        "get_tree" => GET_TREE,
        "get_marks" => GET_MARKS,
        "get_bar_config" => GET_BAR_CONFIG,
        "get_version" => GET_VERSION,
        "get_binding_modes" => GET_BINDING_MODES,
        "get_config" => GET_CONFIG,
        "send_tick" => SEND_TICK,
        _ => return None,
    })
}

/// Event index for a subscribe event name.
pub fn event_index(name: &str) -> Option<u32> {
    Some(match name {
        "workspace" => EV_WORKSPACE,
        "output" => EV_OUTPUT,
        "mode" => EV_MODE,
        "window" => EV_WINDOW,
        "barconfig_update" => EV_BARCONFIG_UPDATE,
        "binding" => EV_BINDING,
        "shutdown" => EV_SHUTDOWN,
        "tick" => EV_TICK,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_layout() {
        let b = encode(RUN_COMMAND, "exit");
        assert_eq!(&b[0..6], b"i3-ipc");
        assert_eq!(&b[6..10], &4u32.to_le_bytes());
        assert_eq!(&b[10..14], &0u32.to_le_bytes());
        assert_eq!(&b[14..], b"exit");
    }
    #[test]
    fn decode_roundtrip() {
        let b = encode(GET_TREE, "{}");
        assert_eq!(decode_header(&b[..14]).unwrap(), (2, 4));
    }
    #[test]
    fn decode_rejects_bad_magic_and_huge_len() {
        assert!(decode_header(b"i4-ipc\0\0\0\0\0\0\0\0").is_err());
        let mut h = encode(0, "").to_vec();
        h[6..10].copy_from_slice(&(MAX_PAYLOAD + 1).to_le_bytes());
        assert!(decode_header(&h).is_err());
        assert!(decode_header(b"short").is_err());
    }
    #[test]
    fn names() {
        assert_eq!(type_from_name("get_version"), Some(7));
        assert_eq!(type_from_name("nope"), None);
        assert_eq!(event_index("tick"), Some(7));
    }
}
