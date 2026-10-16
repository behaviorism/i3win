//! i3 IPC named-pipe server. Connection threads never touch `App`: requests go to
//! the main thread through `mainloop::post`; subscriptions live here.

use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use i3win_core::ipc::{
    decode_header, encode, tick_event, EVENT_BIT, EV_SHUTDOWN, EV_TICK, HEADER_LEN, SUBSCRIBE,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

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

/// Adds a subscription; a repeated SUBSCRIBE on one connection merges its event set.
fn register(subs: &mut Vec<Subscriber>, conn: u64, set: HashSet<String>, tx: &Sender<Vec<u8>>) {
    match subs.iter_mut().find(|(id, _, _)| *id == conn) {
        Some((_, existing, _)) => existing.extend(set),
        None => subs.push((conn, set, tx.clone())),
    }
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

/// An overlapped pipe instance shared by a connection's reader and writer thread.
/// Overlapped handles allow a concurrent read and write; the last owner disconnects
/// and closes it exactly once.
struct PipeHandle(HANDLE);

// SAFETY: a kernel handle value may be used from any thread.
unsafe impl Send for PipeHandle {}
unsafe impl Sync for PipeHandle {}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and this is the only close.
        unsafe {
            let _ = DisconnectNamedPipe(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

/// A manual-reset event owned by one thread, used for its overlapped operations.
struct Event(HANDLE);

impl Event {
    fn new() -> io::Result<Event> {
        // SAFETY: plain Win32 call; no security attributes, unnamed.
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(Event)
            .map_err(io::Error::other)
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: we own the event handle.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Waits for the overlapped operation issued with `ov` and returns the byte count.
fn finish(h: &PipeHandle, ov: &OVERLAPPED) -> io::Result<u32> {
    let mut n = 0u32;
    // SAFETY: `ov` belongs to an operation on `h`; we wait for it to complete.
    unsafe { GetOverlappedResult(h.0, ov, &mut n, true) }.map_err(io::Error::other)?;
    Ok(n)
}

fn read_exact_ov(h: &PipeHandle, ev: &Event, buf: &mut [u8]) -> io::Result<()> {
    let mut off = 0;
    while off < buf.len() {
        let mut ov = OVERLAPPED {
            hEvent: ev.0,
            ..Default::default()
        };
        // SAFETY: `buf` and `ov` stay alive until `finish` returns.
        unsafe {
            let _ = ResetEvent(ev.0);
            if let Err(e) = ReadFile(h.0, Some(&mut buf[off..]), None, Some(&mut ov)) {
                if e.code() != ERROR_IO_PENDING.to_hresult() {
                    return Err(io::Error::other(e));
                }
            }
        }
        let n = finish(h, &ov)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        off += n as usize;
    }
    Ok(())
}

fn write_all_ov(h: &PipeHandle, ev: &Event, buf: &[u8]) -> io::Result<()> {
    let mut off = 0;
    while off < buf.len() {
        let mut ov = OVERLAPPED {
            hEvent: ev.0,
            ..Default::default()
        };
        // SAFETY: `buf` and `ov` stay alive until `finish` returns.
        unsafe {
            let _ = ResetEvent(ev.0);
            if let Err(e) = WriteFile(h.0, Some(&buf[off..]), None, Some(&mut ov)) {
                if e.code() != ERROR_IO_PENDING.to_hresult() {
                    return Err(io::Error::other(e));
                }
            }
        }
        let n = finish(h, &ov)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        off += n as usize;
    }
    Ok(())
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

fn create_instance(wide: &[u16]) -> Option<PipeHandle> {
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
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
            "ipc: CreateNamedPipeW failed: {}",
            windows::core::Error::from_win32()
        );
        return None;
    }
    Some(PipeHandle(handle))
}

fn create_instance_retry(wide: &[u16]) -> PipeHandle {
    loop {
        if let Some(h) = create_instance(wide) {
            return h;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Waits for a client on `pipe` (overlapped).
fn connect(pipe: &PipeHandle, ev: &Event) -> io::Result<()> {
    let mut ov = OVERLAPPED {
        hEvent: ev.0,
        ..Default::default()
    };
    // SAFETY: `ov` stays alive until `finish` returns.
    unsafe {
        let _ = ResetEvent(ev.0);
        if let Err(e) = ConnectNamedPipe(pipe.0, Some(&mut ov)) {
            if e.code() == ERROR_PIPE_CONNECTED.to_hresult() {
                return Ok(());
            }
            if e.code() != ERROR_IO_PENDING.to_hresult() {
                return Err(io::Error::other(e));
            }
        }
    }
    finish(pipe, &ov).map(|_| ())
}

fn accept_loop(pipe_name: &str) {
    let wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    let ev = loop {
        match Event::new() {
            Ok(ev) => break ev,
            Err(e) => {
                log::error!("ipc: cannot create event: {e}");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    };
    let mut cur = create_instance_retry(&wide);
    loop {
        let connected = connect(&cur, &ev);
        // The next instance exists before this one is handed off: no gap for clients.
        let next = create_instance_retry(&wide);
        match connected {
            Ok(()) => spawn_connection(Arc::new(cur)),
            Err(e) => {
                log::warn!("ipc: ConnectNamedPipe failed: {e}");
                drop(cur);
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        cur = next;
    }
}

fn spawn_connection(pipe: Arc<PipeHandle>) {
    let conn = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let wpipe = Arc::clone(&pipe);
    let w = std::thread::Builder::new()
        .name(format!("ipc-w{conn}"))
        .spawn(move || {
            let Ok(ev) = Event::new() else { return };
            for buf in rx {
                if write_all_ov(&wpipe, &ev, &buf).is_err() {
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
                read_loop(conn, &pipe, &tx)
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

fn read_loop(conn: u64, pipe: &PipeHandle, tx: &Sender<Vec<u8>>) {
    let Ok(ev) = Event::new() else { return };
    loop {
        let mut header = [0u8; HEADER_LEN];
        if read_exact_ov(pipe, &ev, &mut header).is_err() {
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
        if read_exact_ov(pipe, &ev, &mut body).is_err() {
            break;
        }
        let payload = String::from_utf8_lossy(&body).into_owned();
        if msg_type == SUBSCRIBE {
            if !handle_subscribe(conn, tx, &payload) {
                break;
            }
            continue;
        }
        let (rtx, rrx) = mpsc::channel();
        mainloop::post(MainMsg::Ipc {
            msg_type,
            payload,
            reply: rtx,
        });
        let reply = rrx
            .recv_timeout(REPLY_TIMEOUT)
            .unwrap_or_else(|_| r#"{"success":false,"error":"timeout"}"#.to_string());
        if tx.send(encode(msg_type, &reply)).is_err() {
            break;
        }
    }
}

/// Handles SUBSCRIBE; returns false when the connection is gone. The reply is queued
/// before the subscriber is registered, so no event can precede it.
fn handle_subscribe(conn: u64, tx: &Sender<Vec<u8>>, payload: &str) -> bool {
    let Ok(names) = serde_json::from_str::<Vec<String>>(payload) else {
        let reply = r#"{"success":false,"error":"invalid subscribe payload"}"#;
        return tx.send(encode(SUBSCRIBE, reply)).is_ok();
    };
    if tx.send(encode(SUBSCRIBE, r#"{"success":true}"#)).is_err() {
        return false;
    }
    let set: HashSet<String> = names.into_iter().collect();
    let wants_tick = set.contains("tick");
    register(&mut lock_subs(), conn, set, tx);
    if wants_tick {
        let frame = encode(EVENT_BIT | EV_TICK, &tick_event(true, ""));
        return tx.send(frame).is_ok();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

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
    fn repeated_subscribe_merges() {
        let (tx, _rx) = mpsc::channel();
        let mut subs = Vec::new();
        register(&mut subs, 1, ["a".to_string()].into_iter().collect(), &tx);
        register(&mut subs, 1, ["b".to_string()].into_iter().collect(), &tx);
        register(&mut subs, 2, ["a".to_string()].into_iter().collect(), &tx);
        assert_eq!(subs.len(), 2);
        assert!(subs[0].1.contains("a") && subs[0].1.contains("b"));
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

    fn read_client_frame(f: &mut std::fs::File) -> io::Result<(u32, String)> {
        let mut h = [0u8; HEADER_LEN];
        f.read_exact(&mut h)?;
        let (len, ty) = decode_header(&h).map_err(io::Error::other)?;
        let mut body = vec![0u8; len as usize];
        f.read_exact(&mut body)?;
        Ok((ty, String::from_utf8_lossy(&body).into_owned()))
    }

    fn open_client(name: &str) -> std::fs::File {
        let end = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
            {
                Ok(f) => return f,
                Err(e) if std::time::Instant::now() >= end => panic!("open {name}: {e}"),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// Frames the client receives, read on a helper thread so the test can time out.
    /// The request must be written first (the client handle is synchronous).
    fn frames_of(mut f: std::fs::File) -> mpsc::Receiver<(u32, String)> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(fr) = read_client_frame(&mut f) {
                if tx.send(fr).is_err() {
                    break;
                }
            }
        });
        rx
    }

    /// Regression: a connection's pending server-side read must not block event
    /// pushes, and replies reach clients that are waiting on them.
    #[test]
    fn event_push_and_reply_while_reader_is_blocked() {
        let _g = mainloop::QUEUE_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _ = mainloop::drain_main_msgs();
        let name = format!(r"\\.\pipe\i3win-ipc-test-{}", std::process::id());
        start(name.clone());

        let mut a = open_client(&name);
        a.write_all(&encode(SUBSCRIBE, r#"["tick"]"#)).unwrap();
        let ra = frames_of(a);
        let t = Duration::from_secs(2);
        assert_eq!(
            ra.recv_timeout(t).unwrap(),
            (SUBSCRIBE, r#"{"success":true}"#.to_string())
        );
        let (ty, p) = ra.recv_timeout(t).unwrap();
        assert_eq!(ty, EVENT_BIT | EV_TICK);
        assert!(p.contains(r#""first":true"#));

        broadcast(EV_TICK, "tick", r#"{"first":false,"payload":"x"}"#);
        let (ty, p) = ra.recv_timeout(t).expect("event must arrive");
        assert_eq!(ty, EVENT_BIT | EV_TICK);
        assert!(p.contains(r#""payload":"x""#));

        let mut b = open_client(&name);
        b.write_all(&encode(7, "")).unwrap();
        let rb = frames_of(b);
        let end = std::time::Instant::now() + t;
        let reply = loop {
            assert!(std::time::Instant::now() < end, "no MainMsg::Ipc posted");
            let found = mainloop::drain_main_msgs()
                .into_iter()
                .find_map(|m| match m {
                    MainMsg::Ipc { reply, .. } => Some(reply),
                    _ => None,
                });
            if let Some(r) = found {
                break r;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        reply.send(r#"{"major":4}"#.to_string()).unwrap();
        assert_eq!(
            rb.recv_timeout(t).unwrap(),
            (7, r#"{"major":4}"#.to_string())
        );
    }
}
