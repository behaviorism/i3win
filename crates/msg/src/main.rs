//! i3win-msg: an i3-msg clone talking to the i3win IPC pipe.

use i3win_core::ipc::{decode_header, encode, EVENT_BIT, HEADER_LEN};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};

#[derive(Debug, PartialEq)]
pub struct Args {
    pub msg_type: u32,
    pub socket: Option<String>,
    pub quiet: bool,
    pub monitor: bool,
    pub payload: String,
}

pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut msg_type = i3win_core::ipc::RUN_COMMAND;
    let mut subscribe = false;
    let mut socket = None;
    let mut quiet = false;
    let mut monitor = false;
    let mut rest: Vec<&str> = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-t" | "--type" => {
                let name = it.next().ok_or("-t requires a message type")?;
                msg_type = i3win_core::ipc::type_from_name(name)
                    .ok_or_else(|| format!("unknown message type: {name}"))?;
                subscribe = name == "subscribe";
            }
            "-s" | "--socket" => {
                socket = Some(it.next().ok_or("-s requires a socket path")?.clone());
            }
            "-q" | "--quiet" => quiet = true,
            "-m" | "--monitor" => monitor = true,
            other => rest.push(other),
        }
    }
    if monitor && !subscribe {
        return Err("monitor requires -t subscribe".into());
    }
    Ok(Args {
        msg_type,
        socket,
        quiet,
        monitor,
        payload: rest.join(" "),
    })
}

/// True if the reply is a JSON array holding an object with `"success": false`.
pub fn any_failed(reply: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(reply) {
        Ok(serde_json::Value::Array(items)) => items
            .iter()
            .any(|v| v.get("success") == Some(&serde_json::Value::Bool(false))),
        _ => false,
    }
}

/// Reads one frame; Ok(None) on clean EOF before a header.
fn read_message(r: &mut impl Read) -> Result<Option<(u32, String)>, String> {
    let mut header = [0u8; HEADER_LEN];
    match r.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    // decode_header rejects payloads over MAX_PAYLOAD before we allocate.
    let (len, ty) = decode_header(&header)?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).map_err(|e| e.to_string())?;
    Ok(Some((ty, String::from_utf8_lossy(&buf).into_owned())))
}

fn run(args: Args) -> Result<i32, String> {
    let name = args
        .socket
        .clone()
        .or_else(|| std::env::var("I3SOCK").ok())
        .unwrap_or_else(|| r"\\.\pipe\i3win-ipc".to_string());
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .map_err(|e| format!("cannot connect to {name}: {e}"))?;
    pipe.write_all(&encode(args.msg_type, &args.payload))
        .map_err(|e| format!("write failed: {e}"))?;
    let (_, reply) = read_message(&mut pipe)?.ok_or("connection closed before reply")?;
    if !args.quiet {
        println!("{reply}");
    }
    if args.monitor {
        while let Some((ty, payload)) = read_message(&mut pipe)? {
            if ty & EVENT_BIT != 0 {
                println!("{payload}");
            }
        }
    }
    Ok(if args.msg_type == 0 && any_failed(&reply) {
        2
    } else {
        0
    })
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("i3win-msg: {e}");
            std::process::exit(1);
        }
    };
    match run(args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("i3win-msg: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    #[test]
    fn defaults_to_command() {
        assert_eq!(
            parse_args(&a(&["workspace", "2"])).unwrap(),
            Args {
                msg_type: 0,
                socket: None,
                quiet: false,
                monitor: false,
                payload: "workspace 2".into()
            }
        );
    }
    #[test]
    fn type_and_socket() {
        let p = parse_args(&a(&["-t", "get_tree", "-s", r"\.\pipe\x"])).unwrap();
        assert_eq!(p.msg_type, 4);
        assert_eq!(p.socket.as_deref(), Some(r"\.\pipe\x"));
    }
    #[test]
    fn monitor_needs_subscribe() {
        assert!(parse_args(&a(&["-m"])).is_err());
        assert!(
            parse_args(&a(&["-t", "subscribe", "-m", r#"["window"]"#]))
                .unwrap()
                .monitor
        );
    }
    #[test]
    fn bad_type() {
        assert!(parse_args(&a(&["-t", "frob"])).is_err());
    }
    #[test]
    fn failure_detection() {
        assert!(any_failed(
            r#"[{"success":true},{"success":false,"error":"x"}]"#
        ));
        assert!(!any_failed(r#"[{"success":true}]"#));
        assert!(!any_failed("{}"));
    }
}
