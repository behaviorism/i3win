//! Spawning block commands: hidden window, piped stdout, all children in one
//! kill-on-close Job Object so they die with i3win-status.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
