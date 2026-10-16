#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod mainloop;
mod paths;
mod reconcile;
mod win;

use std::path::PathBuf;

use simplelog::{
    ColorChoice, CombinedLogger, Config, LevelFilter, SharedLogger, TermLogger, TerminalMode,
    WriteLogger,
};
use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
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

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let mut config_override: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--version" => {
                println!("i3win 0.1.0");
                return;
            }
            "--config" => config_override = args.next().map(PathBuf::from),
            _ => {}
        }
    }

    init_logging();

    // Single instance. The handle is intentionally held until process exit.
    let _mutex = unsafe { CreateMutexW(None, true, w!("Local\\i3win-single-instance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        log::error!("already running");
        std::process::exit(1);
    }

    let config_path = paths::config_path(config_override);

    // Recover windows a crashed previous run left hidden.
    for id in win::hidden::take_previous(&paths::hidden_path()) {
        if win::window::is_window(id) {
            win::window::show(id);
        }
    }

    if let Err(e) = mainloop::create_main_window() {
        log::error!("cannot create main window: {e}");
        std::process::exit(1);
    }
    app::APP.with(|a| *a.borrow_mut() = Some(app::App::start(config_path)));

    // Hooks go in only after the main window exists, so their wake-ups have a target.
    if let Err(e) = win::events::install() {
        log::error!("cannot install event hooks: {e}");
    }
    if let Err(e) = win::keyboard::install() {
        log::error!("cannot install keyboard hook: {e}");
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
