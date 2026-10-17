//! memory: totals via GlobalMemoryStatusEx.

use crate::format::{format_bytes, substitute, Level};
use crate::i3status_conf::{get, Props, Value};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// Threshold like "10%" (of total) or "1G"/"500M" (absolute bytes).
fn threshold_bytes(spec: &str, total: u64) -> Option<f64> {
    let s = spec.trim();
    if let Some(p) = s.strip_suffix('%') {
        return p
            .trim()
            .parse::<f64>()
            .ok()
            .map(|p| total as f64 * p / 100.0);
    }
    let last = s.chars().last()?;
    let (num, mult) = match last.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1024.0),
        'M' => (&s[..s.len() - 1], 1024.0 * 1024.0),
        'G' => (&s[..s.len() - 1], 1024.0 * 1024.0 * 1024.0),
        'T' => (&s[..s.len() - 1], 1024.0 * 1024.0 * 1024.0 * 1024.0),
        _ => (s, 1.0),
    };
    num.trim().parse::<f64>().ok().map(|n| n * mult)
}

pub fn memory_text(total: u64, avail: u64, props: &Props) -> (String, Level) {
    let format = get(props, "format")
        .map(Value::as_str)
        .unwrap_or_else(|| "%used / %total".to_string());
    let prefix = get(props, "prefix_type")
        .map(Value::as_str)
        .unwrap_or_else(|| "binary".to_string());
    let used = total.saturating_sub(avail);
    let pct = |n: u64| {
        if total == 0 {
            0.0
        } else {
            100.0 * n as f64 / total as f64
        }
    };
    let below = |key: &str| {
        get(props, key)
            .and_then(|v| threshold_bytes(&v.as_str(), total))
            .is_some_and(|t| (avail as f64) < t)
    };
    let level = if below("threshold_critical") {
        Level::Bad
    } else if below("threshold_degraded") {
        Level::Degraded
    } else {
        Level::Normal
    };
    let text = substitute(
        &format,
        &[
            ("used", format_bytes(used, &prefix)),
            ("free", format_bytes(avail, &prefix)),
            ("available", format_bytes(avail, &prefix)),
            ("total", format_bytes(total, &prefix)),
            ("percentage_used", format!("{:.1}%", pct(used))),
            ("percentage_free", format!("{:.1}%", pct(avail))),
            ("percentage_available", format!("{:.1}%", pct(avail))),
        ],
    );
    (text, level)
}

/// (total, available) physical bytes.
pub fn read_memory() -> Option<(u64, u64)> {
    let mut m = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()? };
    Some((m.ullTotalPhys, m.ullAvailPhys))
}
