#[allow(dead_code)]
mod discover;
#[allow(dead_code)]
mod i3blocks_conf;
#[allow(dead_code)]
mod i3status_conf;
mod modules;

use modules::*;
use std::io::Write;
use std::time::Duration;

fn emit(out: &mut impl Write, text: &str) -> bool {
    writeln!(out, "{text}").is_ok() && out.flush().is_ok()
}

fn main() {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if !emit(&mut out, r#"{"version":1,"click_events":false}"#) || !emit(&mut out, "[") {
        return;
    }
    let mut prev = read_cpu_times();
    let mut first = true;
    loop {
        let now = read_cpu_times();
        let pct = match (prev, now) {
            (Some(p), Some(n)) => cpu_percent(p, n),
            _ => 0.0,
        };
        prev = now;
        let mut blocks = vec![cpu_block(pct)];
        if let Some(m) = read_mem_used_pct() {
            blocks.push(mem_block(m));
        }
        if let Some(f) = read_disk_free("C:\\") {
            blocks.push(disk_block("C:", f));
        }
        if let Some((p, ac)) = read_battery() {
            blocks.push(battery_block(p, ac));
        }
        blocks.push(time_block(chrono::Local::now()));
        if !emit(&mut out, &render_line(&blocks, first)) {
            return;
        }
        first = false;
        std::thread::sleep(Duration::from_secs(1));
    }
}
