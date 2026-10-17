//! disk: totals via GetDiskFreeSpaceExW.

use crate::format::{format_bytes, substitute, Level};
use crate::i3status_conf::{get, Props, Value};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

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
            let gib = 1024.0 * 1024.0 * 1024.0;
            let value = match kind.as_str() {
                "percentage_free" => pct(free),
                "bytes_free" => free as f64 / gib,
                "bytes_avail" => avail as f64 / gib,
                _ => pct(avail),
            };
            if value < low {
                Level::Bad
            } else {
                Level::Normal
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
