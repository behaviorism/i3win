//! status_command process manager: spawn, read blocks, forward click events.
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use i3win_core::i3bar::{Block, ClickWriter, StatusParser};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

use crate::mainloop::{post, MainMsg};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Source of process generations; messages from a stale reader are ignored by the App.
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

pub struct StatusProcess {
    child: Child,
    /// Feeds the writer thread; click() never blocks on the pipe.
    stdin_tx: Option<Sender<String>>,
    /// Job holding cmd.exe and its descendants; closing it kills the tree.
    job: Option<HANDLE>,
    writer: ClickWriter,
    click_events: Arc<AtomicBool>,
    generation: u64,
}

/// Longest status line kept; the rest of a longer line is dropped.
const MAX_LINE: usize = 64 * 1024;

/// cmd.exe arguments running `command`. With `/S`, cmd strips only the outer
/// quotes, so a command containing quoted paths survives intact.
pub fn cmd_args(command: &str) -> String {
    format!("/S /C \"{command}\"")
}

/// Reads one line into `buf` (without clearing it), keeping at most `cap` bytes
/// and discarding the rest of an over-long line. Returns the bytes consumed;
/// 0 means end of input.
fn read_line_capped<R: BufRead>(
    r: &mut R,
    buf: &mut Vec<u8>,
    cap: usize,
) -> std::io::Result<usize> {
    let mut total = 0;
    loop {
        let (used, done) = {
            let chunk = r.fill_buf()?;
            if chunk.is_empty() {
                return Ok(total);
            }
            let (end, done) = match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => (i + 1, true),
                None => (chunk.len(), false),
            };
            let room = cap.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..end.min(room)]);
            (end, done)
        };
        r.consume(used);
        total += used;
        if done {
            return Ok(total);
        }
    }
}

/// The block shown when the status command is gone or could not start.
pub fn exit_block() -> Block {
    Block {
        full_text: "status_command exited".into(),
        urgent: true,
        separator: true,
        ..Default::default()
    }
}

impl StatusProcess {
    pub fn spawn(command: &str) -> std::io::Result<StatusProcess> {
        let mut cmd = Command::new("cmd.exe");
        cmd.raw_arg(cmd_args(command))
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            let mut paths = vec![dir];
            if let Some(old) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&old));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                cmd.env("PATH", joined);
            }
        }
        let mut child = cmd.spawn()?;
        let job = make_job(&child);
        let (stdin_tx, stdin_rx) = channel::<String>();
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                for line in stdin_rx {
                    if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                        break;
                    }
                }
            });
        }
        let stdout = child.stdout.take();
        let click_events = Arc::new(AtomicBool::new(false));
        let generation = NEXT_GEN.fetch_add(1, Ordering::SeqCst);
        let flag = click_events.clone();
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                let mut parser = StatusParser::new();
                let mut reader = BufReader::new(stdout);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match read_line_capped(&mut reader, &mut buf, MAX_LINE) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let line = String::from_utf8_lossy(&buf);
                    let line = line.trim_end_matches(['\r', '\n']);
                    let blocks = parser.feed_line(line);
                    flag.store(parser.click_events, Ordering::SeqCst);
                    if let Some(b) = blocks {
                        post(MainMsg::StatusUpdate(generation, b));
                    }
                }
            }
            post(MainMsg::StatusExited(generation));
        });
        Ok(StatusProcess {
            child,
            stdin_tx: Some(stdin_tx),
            job,
            writer: ClickWriter::new(),
            click_events,
            generation,
        })
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Forwards a click to the process if it asked for click events; errors are ignored.
    pub fn click(
        &mut self,
        name: Option<&str>,
        instance: Option<&str>,
        button: u8,
        x: i32,
        y: i32,
    ) {
        if !self.click_events.load(Ordering::SeqCst) {
            return;
        }
        let line = self.writer.encode(name, instance, button, x, y);
        if let Some(tx) = self.stdin_tx.as_ref() {
            let _ = tx.send(line);
        }
    }

    pub fn kill(&mut self) {
        self.stdin_tx = None;
        if let Some(job) = self.job.take() {
            // SAFETY: the handle is ours and is not used again.
            unsafe {
                let _ = TerminateJobObject(job, 1);
                let _ = CloseHandle(job);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for StatusProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Creates a kill-on-close job and assigns the child to it. Failure only costs the
/// tree-kill guarantee, so it is logged and ignored.
fn make_job(child: &Child) -> Option<HANDLE> {
    // SAFETY: plain Win32 calls on handles we own.
    unsafe {
        let job = match CreateJobObjectW(None, None) {
            Ok(j) => j,
            Err(e) => {
                log::warn!("status job: {e}");
                return None;
            }
        };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .and_then(|_| AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())));
        if let Err(e) = ok {
            log::warn!("status job: {e}");
            let _ = CloseHandle(job);
            return None;
        }
        Some(job)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mainloop::{drain_main_msgs, MainMsg};

    /// Tests share the global message queue; run them one at a time.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn exit_block_text() {
        let b = exit_block();
        assert_eq!(b.full_text, "status_command exited");
        assert!(b.urgent);
        assert!(b.separator);
    }

    #[test]
    fn spawn_and_read_plain_output() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = StatusProcess::spawn("echo hello").unwrap();
        let gen = p.generation();
        let t = std::time::Instant::now();
        let mut got = vec![];
        while t.elapsed() < std::time::Duration::from_secs(5) {
            got.extend(drain_main_msgs());
            if got
                .iter()
                .any(|m| matches!(m, MainMsg::StatusExited(g) if *g == gen))
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(got.iter().any(
            |m| matches!(m, MainMsg::StatusUpdate(g, b) if *g == gen && b[0].full_text == "hello")
        ));
        p.kill();
    }

    #[test]
    fn kill_terminates_grandchild_holding_the_pipe() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = StatusProcess::spawn("ping -n 30 127.0.0.1").unwrap();
        let gen = p.generation();
        let wait_for = |pred: &dyn Fn(&MainMsg) -> bool| {
            let t = std::time::Instant::now();
            while t.elapsed() < std::time::Duration::from_secs(5) {
                if drain_main_msgs().iter().any(pred) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            false
        };
        assert!(wait_for(
            &|m| matches!(m, MainMsg::StatusUpdate(g, _) if *g == gen)
        ));
        p.kill();
        assert!(wait_for(
            &|m| matches!(m, MainMsg::StatusExited(g) if *g == gen)
        ));
    }
}
