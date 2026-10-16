//! Status modules: pure block builders plus thin Win32 readers.

use chrono::{DateTime, Local};
use i3win_core::i3bar::Block;
use windows::core::PCWSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

fn block(name: &str, text: String) -> Block {
    Block {
        full_text: text,
        name: Some(name.to_string()),
        separator: true,
        ..Block::default()
    }
}

/// `prev`/`now` are (idle, kernel, user) ticks; kernel includes idle.
pub fn cpu_percent(prev: (u64, u64, u64), now: (u64, u64, u64)) -> f64 {
    let di = now.0.saturating_sub(prev.0);
    let dk = now.1.saturating_sub(prev.1);
    let du = now.2.saturating_sub(prev.2);
    let total = dk.saturating_add(du);
    if total == 0 {
        return 0.0;
    }
    let busy = total.saturating_sub(di);
    100.0 * busy as f64 / total as f64
}

pub fn cpu_block(pct: f64) -> Block {
    let mut b = block("cpu", format!("CPU {:>3.0}%", pct));
    if pct >= 90.0 {
        b.color = Some("#ff0000".to_string());
    }
    b
}

pub fn mem_block(used_pct: u32) -> Block {
    let mut b = block("memory", format!("MEM {}%", used_pct));
    if used_pct >= 85 {
        b.color = Some("#ffff00".to_string());
    }
    b
}

pub fn disk_block(drive: &str, free_bytes: u64) -> Block {
    let gib = free_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    block("disk", format!("{drive} {gib:.1} GiB"))
}

pub fn battery_block(percent: u8, ac_online: bool) -> Block {
    let label = if ac_online { "CHR" } else { "BAT" };
    let mut b = block("battery", format!("{label} {percent}%"));
    b.urgent = !ac_online && percent < 15;
    b
}

pub fn time_block(now: DateTime<Local>) -> Block {
    block("time", now.format("%Y-%m-%d %H:%M:%S").to_string())
}

pub fn render_line(blocks: &[Block], first: bool) -> String {
    let json = serde_json::to_string(blocks).unwrap_or_else(|_| "[]".to_string());
    if first {
        json
    } else {
        format!(",{json}")
    }
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

pub fn read_mem_used_pct() -> Option<u32> {
    let mut m = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()? };
    Some(m.dwMemoryLoad)
}

pub fn read_disk_free(root: &str) -> Option<u64> {
    let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None).ok()?;
    }
    Some(free)
}

pub fn read_battery() -> Option<(u8, bool)> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    if s.BatteryFlag == 128 || s.BatteryLifePercent == 255 {
        return None;
    }
    Some((s.BatteryLifePercent, s.ACLineStatus == 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_math() {
        assert!((cpu_percent((100, 300, 100), (150, 400, 200)) - 75.0).abs() < 1e-9);
        assert_eq!(cpu_percent((1, 1, 1), (1, 1, 1)), 0.0);
    }
    #[test]
    fn cpu_colors() {
        assert_eq!(cpu_block(95.0).color.as_deref(), Some("#ff0000"));
        assert_eq!(cpu_block(5.0).full_text, "CPU   5%");
    }
    #[test]
    fn disk_gib() {
        assert_eq!(
            disk_block("C:", 3 * 1024 * 1024 * 1024 / 2).full_text,
            "C: 1.5 GiB"
        );
    }
    #[test]
    fn battery() {
        assert!(battery_block(10, false).urgent);
        assert!(!battery_block(10, true).urgent);
        assert_eq!(battery_block(80, true).full_text, "CHR 80%");
    }
    #[test]
    fn line_prefix() {
        let b = vec![mem_block(40)];
        assert!(render_line(&b, true).starts_with("[{"));
        assert!(render_line(&b, false).starts_with(",[{"));
    }
}
