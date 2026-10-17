//! disk: totals via GetDiskFreeSpaceExW.

use crate::format::{format_bytes, substitute, Level};
use crate::i3status_conf::{get, Props, Value};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// The value compared with `low_threshold`, as i3status: `percentage_*` in
/// percent, `bytes_*` in bytes, `kbytes_*`..`tbytes_*` in units of 1024 (1000
/// with `prefix_type=decimal`); `*_free`/`*_avail` pick the free or
/// caller-available space. `None` for an unknown type (never below).
fn threshold_value(
    kind: &str,
    prefix: &str,
    free: u64,
    avail: u64,
    pct: impl Fn(u64) -> f64,
) -> Option<f64> {
    let kind = kind.to_ascii_lowercase();
    match kind.as_str() {
        "percentage_free" => return Some(pct(free)),
        "percentage_avail" => return Some(pct(avail)),
        _ => {}
    }
    let (scale, which) = match kind.split_once("bytes_")? {
        ("", w) => (0, w),
        ("k", w) => (1, w),
        ("m", w) => (2, w),
        ("g", w) => (3, w),
        ("t", w) => (4, w),
        _ => return None,
    };
    let bytes = match which {
        "free" => free,
        "avail" => avail,
        _ => return None,
    };
    let base: f64 = if prefix.eq_ignore_ascii_case("decimal") {
        1000.0
    } else {
        1024.0
    };
    Some(bytes as f64 / base.powi(scale))
}

pub fn disk_text(total: u64, free: u64, avail: u64, props: &Props) -> (String, Level) {
    let format = get(props, "format")
        .map(Value::as_str)
        .unwrap_or_else(|| "%free".to_string());
    let prefix = get(props, "prefix_type")
        .map(Value::as_str)
        .unwrap_or_else(|| "binary".to_string());
    let used = total.saturating_sub(free);
    let pct = |n: u64| {
        if total == 0 {
            0.0
        } else {
            100.0 * n as f64 / total as f64
        }
    };
    let level = match get(props, "low_threshold").and_then(Value::as_f64) {
        Some(low) => {
            let kind = get(props, "threshold_type")
                .map(Value::as_str)
                .unwrap_or_else(|| "percentage_avail".to_string());
            match threshold_value(&kind, &prefix, free, avail, pct) {
                Some(value) if value < low => Level::Bad,
                _ => Level::Normal,
            }
        }
        None => Level::Normal,
    };
    let text = substitute(
        &format,
        &[
            ("free", format_bytes(free, &prefix)),
            ("used", format_bytes(used, &prefix)),
            ("total", format_bytes(total, &prefix)),
            ("avail", format_bytes(avail, &prefix)),
            ("percentage_free", format!("{:.1}%", pct(free))),
            ("percentage_used", format!("{:.1}%", pct(used))),
            ("percentage_avail", format!("{:.1}%", pct(avail))),
        ],
    );
    (text, level)
}

/// (total, free, available-to-caller) bytes for the volume containing `root`.
pub fn read_disk(root: &str) -> Option<(u64, u64, u64)> {
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut avail),
            Some(&mut total),
            Some(&mut free),
        )
        .ok()?;
    }
    Some((total, free, avail))
}
