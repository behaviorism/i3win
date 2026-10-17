//! cpu_usage: `%usage` from GetSystemTimes deltas.

use crate::format::{substitute, Level};
use crate::i3status_conf::{get, Props, Value};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::GetSystemTimes;

pub fn cpu_text(usage: f64, props: &Props) -> (String, Level) {
    let format = get(props, "format")
        .map(Value::as_str)
        .unwrap_or_else(|| "%usage".to_string());
    let num = |key: &str, default: f64| get(props, key).and_then(Value::as_f64).unwrap_or(default);
    let level = if usage >= num("max_threshold", 95.0) {
        Level::Bad
    } else if usage >= num("degraded_threshold", 90.0) {
        Level::Degraded
    } else {
        Level::Normal
    };
    (
        substitute(&format, &[("usage", format!("{:.0}%", usage))]),
        level,
    )
}

/// `prev`/`now` are (idle, kernel, user) ticks; kernel includes idle.
pub fn cpu_percent(prev: (u64, u64, u64), now: (u64, u64, u64)) -> f64 {
    let di = now.0.saturating_sub(prev.0);
    let total = now
        .1
        .saturating_sub(prev.1)
        .saturating_add(now.2.saturating_sub(prev.2));
    if total == 0 {
        return 0.0;
    }
    100.0 * total.saturating_sub(di) as f64 / total as f64
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

pub fn read_cpu_times() -> Option<(u64, u64, u64)> {
    let (mut i, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).ok()? };
    Some((ft(i), ft(k), ft(u)))
}
