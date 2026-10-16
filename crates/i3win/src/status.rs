//! status_command process manager: spawn, read blocks, forward click events.
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use i3win_core::i3bar::{Block, ClickWriter, StatusParser};

use crate::mainloop::{post, MainMsg};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Source of process generations; messages from a stale reader are ignored by the App.
static NEXT_GEN: AtomicU64 = AtomicU64::new(1);

pub struct StatusProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    writer: ClickWriter,
    click_events: Arc<AtomicBool>,
    generation: u64,
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
        cmd.raw_arg(format!("/C {command}"))
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
        let stdin = child.stdin.take();
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
                    match reader.read_until(b'\n', &mut buf) {
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
            stdin,
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
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = stdin.write_all(line.as_bytes());
            let _ = stdin.flush();
        }
    }

    pub fn kill(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.try_wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mainloop::{drain_main_msgs, MainMsg};

    #[test]
    fn exit_block_text() {
        let b = exit_block();
        assert_eq!(b.full_text, "status_command exited");
        assert!(b.urgent);
        assert!(b.separator);
    }

    #[test]
    fn spawn_and_read_plain_output() {
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
}
