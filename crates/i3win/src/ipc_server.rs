//! i3 IPC named-pipe server. Connection threads never touch `App`: requests go to
//! the main thread through `mainloop::post`; subscriptions live here.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::time::Duration;

use i3win_core::ipc::{
    decode_header, encode, tick_event, EVENT_BIT, EV_SHUTDOWN, EV_TICK, HEADER_LEN, SUBSCRIBE,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

use crate::mainloop::{self, MainMsg};

type Subscriber = (u64, HashSet<String>, Sender<Vec<u8>>);

static SUBSCRIBERS: Mutex<Vec<Subscriber>> = Mutex::new(Vec::new());
static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

fn lock_subs() -> std::sync::MutexGuard<'static, Vec<Subscriber>> {
    SUBSCRIBERS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sends `frame` to every subscriber whose set contains `event_name`; subscribers
/// whose channel is closed are removed. Never blocks (unbounded channels).
fn send_to_matching(subs: &mut Vec<Subscriber>, event_name: &str, frame: &[u8]) {
    subs.retain(|(_, set, tx)| !set.contains(event_name) || tx.send(frame.to_vec()).is_ok());
}

/// Callable on the main thread; never blocks.
pub fn broadcast(event_index: u32, event_name: &str, payload: &str) {
    let frame = encode(EVENT_BIT | event_index, payload);
    send_to_matching(&mut lock_subs(), event_name, &frame);
}

pub fn broadcast_shutdown() {
    broadcast(EV_SHUTDOWN, "shutdown", r#"{"change":"exit"}"#);
    std::thread::sleep(Duration::from_millis(100));
}

/// Spawns the accept thread.
pub fn start(pipe_name: String) {
    let spawned = std::thread::Builder::new()
        .name("ipc-accept".into())
        .spawn(move || accept_loop(&pipe_name));
    if let Err(e) = spawned {
        log::error!("ipc: cannot start accept thread: {e}");
    }
}

fn accept_loop(pipe_name: &str) {
    let wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    loop {
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
        let handle = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                65536,
                65536,
                0,
                None,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            log::error!(
                "ipc: CreateNamedPipeW({pipe_name}) failed: {}",
                windows::core::Error::from_win32()
            );
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        // SAFETY: `handle` is a valid pipe handle we own; the File takes ownership
        // and closes it on drop (also on the failure paths below).
        let file = unsafe { File::from_raw_handle(handle.0 as _) };
        // SAFETY: valid pipe handle, no OVERLAPPED (blocking mode).
        if let Err(e) = unsafe { ConnectNamedPipe(handle, None) } {
            if e.code() != ERROR_PIPE_CONNECTED.to_hresult() {
                log::warn!("ipc: ConnectNamedPipe failed: {e}");
                continue;
            }
        }
        match file.try_clone() {
            Ok(writer) => spawn_connection(file, writer),
            Err(e) => log::warn!("ipc: cannot clone pipe handle: {e}"),
        }
    }
}

fn spawn_connection(reader: File, mut writer: File) {
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let w = std::thread::Builder::new()
        .name(format!("ipc-w{conn}"))
        .spawn(move || {
            for buf in rx {
                if writer.write_all(&buf).is_err() {
                    break;
                }
            }
        });
    if let Err(e) = w {
        log::warn!("ipc: cannot start writer thread: {e}");
        return;
    }
    let r = std::thread::Builder::new()
        .name(format!("ipc-r{conn}"))
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                read_loop(conn, reader, &tx)
            }));
            if result.is_err() {
                log::error!("ipc: connection {conn} thread panicked");
            }
            lock_subs().retain(|(id, _, _)| *id != conn);
        });
    if let Err(e) = r {
        log::warn!("ipc: cannot start reader thread: {e}");
    }
}

fn read_loop(conn: u64, mut file: File, tx: &Sender<Vec<u8>>) {
    loop {
        let mut header = [0u8; HEADER_LEN];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let (len, msg_type) = match decode_header(&header) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("ipc: connection {conn}: {e}");
                break;
            }
        };
        let mut body = vec![0u8; len as usize];
        if file.read_exact(&mut body).is_err() {
            break;
        }
        let payload = String::from_utf8_lossy(&body).into_owned();
        let mut initial_tick = false;
        let reply = if msg_type == SUBSCRIBE {
            let (reply, wants_tick) = subscribe(conn, tx, &payload);
            initial_tick = wants_tick;
            reply
        } else {
            let (rtx, rrx) = mpsc::channel();
            mainloop::post(MainMsg::Ipc {
                msg_type,
                payload,
                reply: rtx,
            });
            rrx.recv_timeout(REPLY_TIMEOUT)
                .unwrap_or_else(|_| r#"{"success":false,"error":"timeout"}"#.to_string())
        };
        if tx.send(encode(msg_type, &reply)).is_err() {
            break;
        }
        if initial_tick {
            let frame = encode(EVENT_BIT | EV_TICK, &tick_event(true, ""));
            if tx.send(frame).is_err() {
                break;
            }
        }
    }
}

/// Registers the subscription; returns the reply and whether `tick` was requested.
fn subscribe(conn: u64, tx: &Sender<Vec<u8>>, payload: &str) -> (String, bool) {
    let Ok(names) = serde_json::from_str::<Vec<String>>(payload) else {
        return (
            r#"{"success":false,"error":"invalid subscribe payload"}"#.to_string(),
            false,
        );
    };
    let set: HashSet<String> = names.into_iter().collect();
    let wants_tick = set.contains("tick");
    lock_subs().push((conn, set, tx.clone()));
    (r#"{"success":true}"#.to_string(), wants_tick)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn sub(id: u64, names: &[&str]) -> (Subscriber, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        let set = names.iter().map(|s| s.to_string()).collect();
        ((id, set, tx), rx)
    }

    #[test]
    fn only_matching_subscribers_receive() {
        let (a, ra) = sub(1, &["workspace"]);
        let (b, rb) = sub(2, &["window", "tick"]);
        let mut subs = vec![a, b];
        send_to_matching(&mut subs, "workspace", b"frame");
        assert_eq!(ra.try_recv().unwrap(), b"frame");
        assert!(rb.try_recv().is_err());
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn failed_send_drops_subscriber() {
        let (a, ra) = sub(1, &["tick"]);
        let (b, rb) = sub(2, &["tick"]);
        drop(ra);
        let mut subs = vec![a, b];
        send_to_matching(&mut subs, "tick", b"x");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].0, 2);
        assert_eq!(rb.try_recv().unwrap(), b"x");
    }
}
