//! volume: default render endpoint via Core Audio. The `volume NAME` instance is
//! ignored; the default device is always used.

use crate::format::{substitute, Level};
use crate::i3status_conf::{get, Props, Value};
use std::cell::Cell;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};

pub fn volume_text(level: Option<(u32, bool)>, props: &Props) -> (String, Level) {
    let text_of = |key: &str, default: &str| {
        get(props, key)
            .map(Value::as_str)
            .unwrap_or_else(|| default.to_string())
    };
    let Some((vol, muted)) = level else {
        return ("♪: --".to_string(), Level::Degraded);
    };
    let vars = [("volume", format!("{vol}%"))];
    if muted {
        (
            substitute(&text_of("format_muted", "♪: muted (%volume)"), &vars),
            Level::Degraded,
        )
    } else {
        (
            substitute(&text_of("format", "♪: %volume"), &vars),
            Level::Normal,
        )
    }
}

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

fn ensure_com() {
    COM_READY.with(|ready| {
        if !ready.get() {
            // S_FALSE (already initialised) and RPC_E_CHANGED_MODE (other apartment
            // model) are both fine: COM is usable on this thread either way.
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            ready.set(true);
        }
    });
}

/// (volume 0-100, muted) of the default render device; None on any failure.
pub fn read_volume() -> Option<(u32, bool)> {
    ensure_com();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        let scalar = endpoint.GetMasterVolumeLevelScalar().ok()?;
        let muted = endpoint.GetMute().ok()?.as_bool();
        Some(((scalar * 100.0).round().clamp(0.0, 100.0) as u32, muted))
    }
}
