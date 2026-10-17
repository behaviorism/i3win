//! Spawning block commands: hidden window, piped stdout, all children in one
//! kill-on-close Job Object so they die with i3win-status.

use std::io::{self, BufRead};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Longest output line kept (as v1); the rest of a longer line is dropped.
pub const MAX_LINE: usize = 64 * 1024;

/// Reads one line (without its `\r\n`/`\n`), keeping at most `cap` bytes and
/// discarding the rest of an over-long line. `None` at end of input.
pub fn read_line_capped<R: BufRead>(r: &mut R, cap: usize) -> Option<String> {
    let mut buf = Vec::new();
    let mut any = false;
    loop {
        let (used, done) = {
            let chunk = match r.fill_buf() {
                Ok(c) => c,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => &[],
            };
            if chunk.is_empty() {
                break;
            }
            any = true;
            let (part, done) = match chunk.iter().position(|&b| b == b'\n') {
                Some(n) => (&chunk[..n], true),
                None => (chunk, false),
            };
            let room = cap.saturating_sub(buf.len());
            buf.extend_from_slice(&part[..part.len().min(room)]);
            (part.len() + usize::from(done), done)
        };
        r.consume(used);
        if done {
            break;
        }
    }
    if !any {
        return None;
    }
    let capped = buf.len() >= cap;
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Some(decode_line(&buf, capped, oem_decode))
}

/// One output line as text: UTF-8 when valid, else (console tools, cmd's
/// `echo`, PowerShell) decoded by `fallback`, the OEM code page. When the line
/// was `capped`, a UTF-8 character cut off at the end is dropped rather than
/// sending the whole line to `fallback`.
pub fn decode_line(bytes: &[u8], capped: bool, fallback: impl Fn(&[u8]) -> String) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(e) if capped && e.error_len().is_none() => {
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned()
        }
        Err(_) => fallback(bytes),
    }
}

/// `bytes` decoded from the OEM code page (lossy UTF-8 if that fails).
pub fn oem_decode(bytes: &[u8]) -> String {
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    // SAFETY: the slices are valid for the duration of each call.
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, flags, bytes, None);
        if n <= 0 {
            return String::from_utf8_lossy(bytes).into_owned();
        }
        let mut wide = vec![0u16; n as usize];
        let n = MultiByteToWideChar(CP_OEMCP, flags, bytes, Some(&mut wide));
        String::from_utf16_lossy(&wide[..n.max(0) as usize])
    }
}

/// A Job Object that kills every process in it when its last handle closes
/// (that is, when i3win-status exits). The handle is intentionally never
/// closed. `None` only costs the tree-kill guarantee.
pub fn create_kill_job() -> Option<HANDLE> {
    // SAFETY: plain Win32 calls on a handle we own.
    unsafe {
        let job = CreateJobObjectW(None, None).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if set.is_err() {
            let _ = CloseHandle(job);
            return None;
        }
        Some(job)
    }
}

/// Starts one block command. `raw_cmdline` passes `args` verbatim (cmd.exe's
/// own quoting); `env` is added to the inherited environment. stdin is a pipe
/// for persist blocks (clicks), null otherwise; stdout is piped; stderr null.
pub fn spawn_block(
    prog: &str,
    args: &[String],
    raw_cmdline: bool,
    env: &[(String, String)],
    job: HANDLE,
    persist: bool,
) -> io::Result<Child> {
    let mut cmd = Command::new(prog);
    for a in args {
        if raw_cmdline {
            cmd.raw_arg(a);
        } else {
            cmd.arg(a);
        }
    }
    cmd.envs(env.iter().map(|(k, v)| (k, v)))
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(if persist {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let child = cmd.spawn()?;
    if !job.is_invalid() {
        // SAFETY: both handles are valid for the duration of the call.
        let _ = unsafe { AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())) };
    }
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn lines_are_capped() {
        let text = format!("short\r\n{}\nlast", "y".repeat(100));
        let mut r = io::BufReader::with_capacity(16, text.as_bytes());
        assert_eq!(read_line_capped(&mut r, 10).as_deref(), Some("short"));
        assert_eq!(read_line_capped(&mut r, 10), Some("y".repeat(10)));
        assert_eq!(read_line_capped(&mut r, 10).as_deref(), Some("last"));
        assert_eq!(read_line_capped(&mut r, 10), None);
        let mut empty = io::BufReader::new("\n".as_bytes());
        assert_eq!(read_line_capped(&mut empty, 10).as_deref(), Some(""));
        assert_eq!(read_line_capped(&mut empty, 10), None);
    }

    #[test]
    fn non_utf8_lines_use_the_fallback_decoder() {
        let fake = |b: &[u8]| format!("oem:{}", b.len());
        assert_eq!(
            decode_line("caf\u{e9}".as_bytes(), false, fake),
            "caf\u{e9}"
        );
        assert_eq!(decode_line(b"caf\x82", false, fake), "oem:4");
        // A UTF-8 char cut by the line cap is dropped, the rest stays UTF-8.
        assert_eq!(decode_line(b"ab\xc3", true, fake), "ab");
        assert_eq!(decode_line(b"ab\xc3", false, fake), "oem:3");
        // A real OEM decode never yields U+FFFD for single-byte text.
        assert!(!oem_decode(b"caf\x82").contains('\u{fffd}'));
        assert_eq!(oem_decode(b"plain"), "plain");
        assert_eq!(oem_decode(b""), "");
    }

    #[test]
    fn non_utf8_line_is_read_via_oem() {
        let mut r = io::BufReader::new(&b"caf\x82\r\nok\n"[..]);
        let line = read_line_capped(&mut r, 100).unwrap();
        assert!(
            line.starts_with("caf") && !line.contains('\u{fffd}'),
            "{line}"
        );
        assert_eq!(read_line_capped(&mut r, 100).as_deref(), Some("ok"));
    }

    #[test]
    fn spawn_echo_with_env() {
        let job = create_kill_job().unwrap_or_default();
        let (prog, args, raw) = crate::command_block::shell_command("cmd", "echo %GREETING%");
        let env = vec![("GREETING".to_string(), "hello there".to_string())];
        let mut child = spawn_block(&prog, &args, raw, &env, job, false).unwrap();
        let mut out = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out.trim(), "hello there");
        assert_eq!(child.wait().unwrap().code(), Some(0));
    }

    #[test]
    fn exit_code_is_reported() {
        let job = create_kill_job().unwrap_or_default();
        let (prog, args, raw) = crate::command_block::shell_command("cmd", "exit 33");
        let mut child = spawn_block(&prog, &args, raw, &[], job, false).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(33));
    }
}
