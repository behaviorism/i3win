#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bar_layout;
mod drag;
mod hover;
mod ipc_server;
mod mainloop;
mod paths;
mod reconcile;
mod status;
mod win;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread::ThreadId;

use simplelog::{
    ColorChoice, CombinedLogger, Config, LevelFilter, SharedLogger, TermLogger, TerminalMode,
    WriteLogger,
};
use windows::core::w;
use windows::Win32::Foundation::{GetLastError, BOOL, ERROR_ALREADY_EXISTS, FALSE, TRUE};
use windows::Win32::System::Console::{
    SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_CLOSE_EVENT, CTRL_C_EVENT, CTRL_LOGOFF_EVENT,
    CTRL_SHUTDOWN_EVENT,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

fn init_logging() {
    let level = if std::env::var("I3WIN_LOG").is_ok_and(|v| v == "debug") {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let mut loggers: Vec<Box<dyn SharedLogger>> = Vec::new();
    if let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths::log_path())
    {
        loggers.push(WriteLogger::new(level, Config::default(), f));
    }
    if cfg!(debug_assertions) {
        loggers.push(TermLogger::new(
            level,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ));
    }
    let _ = CombinedLogger::init(loggers);
}

/// Runs on a system-created thread: touches only the hidden mirror and the main queue.
unsafe extern "system" fn console_ctrl_handler(ctrl: u32) -> BOOL {
    match ctrl {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
        | CTRL_SHUTDOWN_EVENT => {
            win::hidden::restore_all_hidden();
            mainloop::post(mainloop::MainMsg::Shutdown);
            TRUE
        }
        _ => FALSE,
    }
}

static MAIN_THREAD: OnceLock<ThreadId> = OnceLock::new();

fn install_crash_safety() {
    let _ = MAIN_THREAD.set(std::thread::current().id());
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Only a panic on the main thread poisons the app; restore first, then log.
        if MAIN_THREAD.get() == Some(&std::thread::current().id()) {
            win::hidden::restore_all_hidden();
        }
        log::error!("panic: {info}");
        default_hook(info);
    }));
    // SAFETY: the handler is a plain `extern "system"` fn that lives for the whole process.
    if let Err(e) = unsafe { SetConsoleCtrlHandler(Some(console_ctrl_handler), true) } {
        log::warn!("cannot install console ctrl handler: {e}");
    }
}

#[cfg(debug_assertions)]
fn spawn_debug_panic_trigger() {
    let Some(secs) = std::env::var("I3WIN_PANIC_AFTER_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return;
    };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        mainloop::post(mainloop::MainMsg::DebugPanic);
    });
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let mut config_override: Option<PathBuf> = None;
    let mut ipc_pipe = String::from(r"\\.\pipe\i3win-ipc");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" => {
                println!("i3win {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--config" => config_override = args.next().map(PathBuf::from),
            "--ipc-pipe" => {
                if let Some(name) = args.next() {
                    ipc_pipe = name;
                }
            }
            _ => {}
        }
    }

    // Before any thread or child exists, so status_command scripts inherit it too.
    std::env::set_var("I3SOCK", &ipc_pipe);

    init_logging();
    // Before first use of the shell COM objects (cloaking) on this thread.
    win::cloak::init_com();
    install_crash_safety();
    #[cfg(debug_assertions)]
    spawn_debug_panic_trigger();

    // Single instance. The handle is intentionally held until process exit.
    let _mutex = unsafe { CreateMutexW(None, true, w!("Local\\i3win-single-instance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        log::error!("already running");
        std::process::exit(1);
    }

    let config_path = paths::config_path(config_override);

    if let Err(e) = mainloop::create_main_window() {
        log::error!("cannot create main window: {e}");
        std::process::exit(1);
    }
    ipc_server::start(ipc_pipe.clone());
    // Hooks go in as soon as the main window exists (their wake-ups need a target),
    // so windows shown by the recovery below are seen as late Shown events.
    if let Err(e) = win::events::install() {
        log::error!("cannot install event hooks: {e}");
    }

    // Recover windows a crashed previous run left hidden. Only windows that are
    // still owned by the same process and are still invisible (HWNDs get reused).
    for (id, pid) in win::hidden::take_previous(&paths::hidden_path()) {
        if win::window::is_window(id) && win::window::window_pid(id) == pid {
            // A cloaked window is still "visible", so uncloak whatever the state.
            win::cloak::uncloak_fresh(id);
            if !win::window::is_visible(id) {
                win::window::show(id);
            }
        }
    }

    app::APP.with(|a| *a.borrow_mut() = Some(app::App::start(config_path, ipc_pipe)));
    if let Err(e) = win::keyboard::install() {
        log::error!("cannot install keyboard hook: {e}");
    }
    if let Err(e) = win::mouse::install() {
        log::error!("cannot install mouse hook: {e}");
    }
    app::APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            for ev in win::events::drain() {
                app.on_event(ev);
            }
            app.flush();
        }
    });

    let code = mainloop::run();
    app::APP.with(|a| {
        if let Some(app) = a.borrow_mut().as_mut() {
            app.shutdown();
        }
    });
    std::process::exit(code);
}
