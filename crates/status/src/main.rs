mod command_block;
mod discover;
mod format;
mod i3blocks_conf;
mod i3status_conf;
mod mods;
mod process;
mod runner;
mod signal;

use command_block::{block_env, exit_code_block, expand_sh_vars, is_cmd_shell, shell_command};
use discover::{detect_format, discover, Format};
use i3blocks_conf::Section;
use i3status_conf::{
    get, module_props, parse_i3status, ModuleRef, Props, Value, DEFAULT_I3STATUS_CONF,
};
use i3win_core::i3bar::{Block, Click};
use runner::{
    config_error_block, module_interval, persist_click_line, persist_line_block, plain_line,
    push_capped, render_line, run_block, slot_for_click, ClickAction, Coalescer, CommandSlot,
    Schedule, Slot,
};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;

/// i3status' default `general.interval`.
const DEFAULT_INTERVAL: f64 = 5.0;
const TICK: Duration = Duration::from_millis(100);

pub(crate) enum Event {
    Tick,
    /// (slot, run, line)
    Output(usize, u64, String),
    /// (slot, run, exit code)
    Exited(usize, u64, i32),
    /// (slot, run): the command could not be started.
    SpawnFailed(usize, u64),
    Click(Click),
    /// stdin closed: the bar went away.
    Quit,
    /// `--signal N` delivery.
    Signal(u32),
    /// `--refresh-all` delivery.
    RefreshAll,
}

#[derive(Debug, Default, PartialEq)]
struct Args {
    config: Option<PathBuf>,
    signal: Option<u32>,
    refresh_all: bool,
}

/// `-c PATH`, `--signal N`, `--refresh-all`; other arguments are ignored.
/// `--signal` without a valid number is an error.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "-c" {
            a.config = it.next().map(PathBuf::from);
        } else if arg == "--signal" {
            let n = it
                .next()
                .ok_or_else(|| "--signal needs a signal number".to_string())?;
            let sig = n
                .trim()
                .parse::<u32>()
                .map_err(|_| format!("--signal: invalid signal number `{n}`"))?;
            a.signal = Some(sig);
        } else if arg == "--refresh-all" {
            a.refresh_all = true;
        }
    }
    Ok(a)
}

#[derive(Debug, PartialEq)]
enum OutputMode {
    I3Bar,
    /// `output_format = "none"`: plain lines joined by this separator.
    Plain(String),
}

struct Setup {
    schedule: Schedule,
    output: OutputMode,
    interval: f64,
}

fn schedule_of(slots: Vec<Slot>) -> Schedule {
    let blocks = slots
        .iter()
        .map(|s| match s {
            Slot::Static(b) => b.clone(),
            Slot::Command(c) => command_block::static_block(&c.section),
            Slot::Module(m) => {
                let mut b = Block::new("");
                b.name = Some(m.name().to_string());
                b.instance = m.instance().map(str::to_string);
                b
            }
        })
        .collect();
    Schedule { slots, blocks }
}

fn error_slots(errors: &[(usize, String)]) -> Vec<Slot> {
    errors
        .iter()
        .map(|(line, msg)| {
            eprintln!("i3win-status: config line {line}: {msg}");
            Slot::Static(config_error_block(*line))
        })
        .collect()
}

fn setup_i3status(text: &str) -> Setup {
    let cfg = parse_i3status(text);
    let mut slots = error_slots(&cfg.errors);
    for m in &cfg.order {
        let props = module_props(&cfg, m).cloned().unwrap_or_default();
        slots.push(Slot::Module(mods::build_module(m, &props, &cfg.general)));
    }
    let interval = get(&cfg.general, "interval")
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(DEFAULT_INTERVAL);
    let output = match get(&cfg.general, "output_format").map(Value::as_str) {
        Some(f) if f == "none" => OutputMode::Plain(
            get(&cfg.general, "separator")
                .map(Value::as_str)
                .unwrap_or_else(|| " | ".to_string()),
        ),
        _ => OutputMode::I3Bar,
    };
    Setup {
        schedule: schedule_of(slots),
        output,
        interval,
    }
}

#[derive(Debug, PartialEq)]
pub enum SectionKind {
    Command,
    Module(ModuleRef),
    Static,
}

const KNOWN_MODULES: [&str; 11] = [
    "cpu_usage",
    "memory",
    "disk",
    "battery",
    "tztime",
    "time",
    "ethernet",
    "wireless",
    "volume",
    "path_exists",
    "read_file",
];

/// A section with a `command` runs it; otherwise a built-in module name
/// (optionally followed by an instance) selects a module.
pub fn classify_section(s: &Section) -> SectionKind {
    if s.get("command").is_some_and(|c| !c.trim().is_empty()) {
        return SectionKind::Command;
    }
    let (module, instance) = match s.name.split_once(' ') {
        Some((m, i)) => (m, Some(i.trim().to_string()).filter(|i| !i.is_empty())),
        None => (s.name.as_str(), None),
    };
    if KNOWN_MODULES.contains(&module) {
        SectionKind::Module(ModuleRef {
            module: module.to_string(),
            instance,
        })
    } else {
        SectionKind::Static
    }
}

/// The section's i3blocks values (globals included) as module properties.
pub fn section_props(s: &Section) -> Props {
    s.props
        .iter()
        .map(|(k, v)| (k.clone(), Value::Str(v.clone())))
        .collect()
}

fn setup_i3blocks(text: &str) -> Setup {
    let cfg = i3blocks_conf::parse_i3blocks(text);
    let mut slots = error_slots(&cfg.errors);
    for s in cfg.sections {
        slots.push(match classify_section(&s) {
            SectionKind::Command => Slot::Command(CommandSlot::from_section(s)),
            SectionKind::Module(m) => {
                let props = section_props(&s);
                let inner = mods::build_module(&m, &props, &props);
                Slot::Module(Box::new(mods::SectionModule { inner, section: s }))
            }
            SectionKind::Static => Slot::Static(command_block::static_block(&s)),
        });
    }
    Setup {
        schedule: schedule_of(slots),
        output: OutputMode::I3Bar,
        interval: DEFAULT_INTERVAL,
    }
}

fn read_lossy(path: &Path) -> std::io::Result<String> {
    std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Finds, reads (or creates) the config. Returns the setup and the config's
/// folder (`SCRIPT_DIR`).
fn load(args: &Args) -> (Setup, PathBuf) {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let env_cfg = std::env::var("I3BLOCKS_CONFIG").ok();
    let (path, fmt) = discover(
        args.config.as_deref(),
        env_cfg.as_deref(),
        &home,
        &|p: &Path| p.is_file(),
    );
    let path = std::path::absolute(&path).unwrap_or(path);
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let text = match read_lossy(&path) {
        Ok(t) => t,
        Err(_) if args.config.is_none() && !path.exists() => {
            // Nothing found: write the default i3status config and use it.
            let wrote = std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::write(&path, DEFAULT_I3STATUS_CONF));
            if let Err(e) = wrote {
                eprintln!("i3win-status: cannot write {}: {e}", path.display());
            }
            DEFAULT_I3STATUS_CONF.to_string()
        }
        Err(e) => {
            eprintln!("i3win-status: cannot read {}: {e}", path.display());
            let mut b = Block::new(format!("config error: cannot read {}", path.display()));
            b.color = Some("#FF0000".to_string());
            let setup = Setup {
                schedule: schedule_of(vec![Slot::Static(b)]),
                output: OutputMode::I3Bar,
                interval: DEFAULT_INTERVAL,
            };
            return (setup, dir);
        }
    };
    let setup = match fmt.unwrap_or_else(|| detect_format(&text)) {
        Format::I3Status => setup_i3status(&text),
        Format::I3Blocks => setup_i3blocks(&text),
    };
    (setup, dir)
}

/// How long a finished child's buffered output may take to drain before the
/// run is reported finished.
const DRAIN: Duration = Duration::from_millis(150);

/// Watches one run: a reader thread forwards stdout lines; this thread waits
/// for the process to exit (not for stdout EOF, which a background grandchild
/// holding the pipe could delay forever), lets the reader drain briefly, then
/// reports the exit code.
fn watch_child(i: usize, run: u64, mut child: std::process::Child, tx: Sender<Event>) {
    let (done_tx, done_rx) = channel::<()>();
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut r = BufReader::new(out);
            while let Some(line) = process::read_line_capped(&mut r, process::MAX_LINE) {
                if tx.send(Event::Output(i, run, line)).is_err() {
                    break;
                }
            }
            let _ = done_tx.send(());
        });
    } else {
        drop(done_tx);
    }
    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
    let _ = done_rx.recv_timeout(DRAIN);
    let _ = tx.send(Event::Exited(i, run, code));
}

/// Parses i3bar click lines from stdin (skipping the leading `[` and `,`).
fn read_clicks(tx: Sender<Event>) {
    let stdin = std::io::stdin();
    let mut r = stdin.lock();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match r.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => {
                // The bar is gone.
                let _ = tx.send(Event::Quit);
                return;
            }
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf);
                let t = line.trim().trim_start_matches(['[', ',']).trim();
                if t.is_empty() {
                    continue;
                }
                if let Ok(c) = serde_json::from_str::<Click>(t) {
                    if tx.send(Event::Click(c)).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

struct Runner {
    sched: Schedule,
    interval: f64,
    module_next: Vec<f64>,
    /// Output collected during the current run of each non-persist slot.
    pending: Vec<Vec<String>>,
    /// Bytes in `pending[i]`.
    pending_size: Vec<usize>,
    /// Click line feeds of running persist slots.
    stdin: Vec<Option<Sender<String>>>,
    script_dir: PathBuf,
    job: HANDLE,
    tx: Sender<Event>,
    start: Instant,
    writer: Coalescer,
}

impl Runner {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn set_block(&mut self, i: usize, b: Block) {
        if self.sched.set_block(i, b) {
            self.writer.dirty = true;
        }
    }

    fn spawn(&mut self, i: usize, click: Option<&Click>) {
        let Some(c) = self.sched.command(i) else {
            return;
        };
        let persist = c.is_persist();
        let env = block_env(&c.section, &self.script_dir, click);
        let shell = c.section.get("shell").unwrap_or("");
        let cmdline = if is_cmd_shell(shell) {
            expand_sh_vars(c.command(), &env, std::process::id())
        } else {
            c.command().to_string()
        };
        let (prog, args, raw) = shell_command(shell, &cmdline);
        let run = self.sched.on_started(i);
        self.pending[i].clear();
        self.pending_size[i] = 0;
        match process::spawn_block(&prog, &args, raw, &env, self.job, persist) {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let (tx, rx) = channel::<String>();
                    self.stdin[i] = Some(tx);
                    std::thread::spawn(move || {
                        for line in rx {
                            if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                                return;
                            }
                        }
                    });
                }
                let tx = self.tx.clone();
                std::thread::spawn(move || watch_child(i, run, child, tx));
            }
            Err(e) => {
                eprintln!("i3win-status: cannot start `{prog}`: {e}");
                let _ = self.tx.send(Event::SpawnFailed(i, run));
            }
        }
    }

    fn tick(&mut self) {
        let now = self.now();
        for i in 0..self.sched.slots.len() {
            if now < self.module_next[i] {
                continue;
            }
            if let Slot::Module(m) = &mut self.sched.slots[i] {
                let b = m.render();
                self.module_next[i] = now + module_interval(m.as_ref(), self.interval);
                self.set_block(i, b);
            }
        }
        for i in self.sched.due(now) {
            self.spawn(i, None);
        }
    }

    fn output(&mut self, i: usize, run: u64, line: String) {
        if !self.sched.is_current(i, run) {
            return; // late output of a finished run
        }
        let Some(c) = self.sched.command(i) else {
            return;
        };
        if c.is_persist() {
            let b = persist_line_block(&c.section, c.format, &line);
            self.set_block(i, b);
        } else {
            push_capped(&mut self.pending[i], &mut self.pending_size[i], line);
        }
    }

    /// A run ended with `code`; `spawn_failed` when it never started (shown
    /// as code 127, what sh reports for a missing command, and retried no
    /// sooner than [`runner::SPAWN_RETRY`]).
    fn exited(&mut self, i: usize, run: u64, code: i32, spawn_failed: bool) {
        if !self.sched.is_current(i, run) {
            return;
        }
        let Some(c) = self.sched.command(i) else {
            return;
        };
        let b = if c.is_persist() {
            let mut b = self.sched.blocks[i].clone();
            exit_code_block(&mut b, c.command(), code);
            b
        } else {
            run_block(&c.section, c.format, &self.pending[i].join("\n"), code)
        };
        self.pending[i].clear();
        self.pending_size[i] = 0;
        self.stdin[i] = None;
        if spawn_failed {
            self.sched.on_spawn_failed(i, self.now());
        } else {
            self.sched.on_finished(i, self.now());
        }
        self.set_block(i, b);
        // A `repeat` block is due again at once.
        for j in self.sched.due(self.now()) {
            self.spawn(j, None);
        }
    }

    fn click(&mut self, c: Click) {
        let Some(i) = slot_for_click(&self.sched, c.name.as_deref(), c.instance.as_deref()) else {
            return;
        };
        match self.sched.on_click(i) {
            ClickAction::Spawn => self.spawn(i, Some(&c)),
            ClickAction::SendToStdin => {
                if let (Some(tx), Some(slot)) = (&self.stdin[i], self.sched.command(i)) {
                    let _ = tx.send(persist_click_line(slot.format, &c));
                }
            }
            ClickAction::Drop => {}
        }
    }

    /// Handles one event; false means quit.
    fn handle(&mut self, ev: Event) -> bool {
        match ev {
            Event::Quit => return false,
            Event::Tick => self.tick(),
            Event::Output(i, run, line) => self.output(i, run, line),
            Event::Exited(i, run, code) => self.exited(i, run, code, false),
            Event::SpawnFailed(i, run) => self.exited(i, run, 127, true),
            Event::Click(c) => self.click(c),
            Event::Signal(sig) => {
                for i in self.sched.on_signal(sig) {
                    self.spawn(i, None);
                }
            }
            Event::RefreshAll => {
                for i in self.sched.refresh_all() {
                    self.spawn(i, None);
                }
            }
        }
        true
    }
}

fn emit(out: &mut impl Write, text: &str) -> bool {
    writeln!(out, "{text}").is_ok() && out.flush().is_ok()
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("i3win-status: {e}");
            std::process::exit(2);
        }
    };
    if args.signal.is_some() || args.refresh_all {
        let n = match args.signal {
            Some(sig) => signal::send_signal(sig),
            None => signal::send_refresh_all(),
        };
        eprintln!("{n}");
        return;
    }
    let (setup, script_dir) = load(&args);
    let job = process::create_kill_job().unwrap_or_else(|| {
        eprintln!("i3win-status: cannot create job object; children may outlive us");
        HANDLE::default()
    });
    let (tx, rx) = channel::<Event>();
    let n = setup.schedule.slots.len();
    let mut r = Runner {
        sched: setup.schedule,
        interval: setup.interval,
        module_next: vec![0.0; n],
        pending: vec![Vec::new(); n],
        pending_size: vec![0; n],
        stdin: vec![None; n],
        script_dir,
        job,
        tx: tx.clone(),
        start: Instant::now(),
        writer: Coalescer::default(),
    };
    r.writer.dirty = true;

    let mut out = std::io::stdout();
    if setup.output == OutputMode::I3Bar
        && (!emit(&mut out, r#"{"version":1,"click_events":true}"#) || !emit(&mut out, "["))
    {
        return;
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while tx.send(Event::Tick).is_ok() {
                std::thread::sleep(TICK);
            }
        });
    }
    signal::listen(tx.clone());
    std::thread::spawn(move || read_clicks(tx));

    let mut first = true;
    r.tick();
    loop {
        let wait = if r.writer.dirty {
            r.writer.wait(r.now())
        } else {
            1.0
        };
        match rx.recv_timeout(Duration::from_secs_f64(wait)) {
            Ok(ev) => {
                if !r.handle(ev) {
                    return; // the Job Object takes the children down
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if r.writer.take(r.now()) {
            let line = match &setup.output {
                OutputMode::I3Bar => render_line(&r.sched.blocks, first),
                OutputMode::Plain(sep) => plain_line(&r.sched.blocks, sep),
            };
            if !emit(&mut out, &line) {
                return; // the bar went away
            }
            first = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_config_path() {
        let a = parse_args(["-c".to_string(), r"C:\x.conf".to_string()]).unwrap();
        assert_eq!(a.config, Some(PathBuf::from(r"C:\x.conf")));
        assert!(
            parse_args(["--refresh-all".to_string()])
                .unwrap()
                .refresh_all
        );
        assert_eq!(
            parse_args(["--signal".to_string(), "5".to_string()])
                .unwrap()
                .signal,
            Some(5)
        );
        assert_eq!(parse_args(["-c".to_string()]), Ok(Args::default()));
    }

    #[test]
    fn args_signal_needs_a_number() {
        assert!(parse_args(["--signal".to_string()]).is_err());
        assert!(parse_args(["--signal".to_string(), "abc".to_string()]).is_err());
        assert!(parse_args(["--signal".to_string(), "-1".to_string()]).is_err());
        let e = parse_args(["--signal".to_string(), "abc".to_string()]).unwrap_err();
        assert!(e.contains("abc"), "{e}");
    }

    fn sec(name: &str, props: &[(&str, &str)]) -> Section {
        Section {
            name: name.to_string(),
            props: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn classify_sections() {
        assert_eq!(
            classify_section(&sec("weather", &[("command", "x")])),
            SectionKind::Command
        );
        assert_eq!(
            classify_section(&sec("disk C:", &[("format", "%free")])),
            SectionKind::Module(ModuleRef {
                module: "disk".into(),
                instance: Some("C:".into())
            })
        );
        assert_eq!(
            classify_section(&sec("cpu_usage", &[])),
            SectionKind::Module(ModuleRef {
                module: "cpu_usage".into(),
                instance: None
            })
        );
        assert_eq!(
            classify_section(&sec("hello", &[("full_text", "hi")])),
            SectionKind::Static
        );
    }

    #[test]
    fn i3blocks_setup_errors_first() {
        let s = setup_i3blocks("[a]\nfull_text=x\nbad line\n[b]\ncommand=echo hi\ninterval=1\n");
        let blocks = &s.schedule.blocks;
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].full_text, "config error line 3");
        assert_eq!(blocks[0].color.as_deref(), Some("#FF0000"));
        assert_eq!(blocks[1].full_text, "x");
        assert!(matches!(s.schedule.slots[1], Slot::Static(_)));
        assert!(matches!(s.schedule.slots[2], Slot::Command(_)));
        assert_eq!(blocks[2].name.as_deref(), Some("b"));
        assert_eq!(s.output, OutputMode::I3Bar);
    }

    #[test]
    fn i3status_setup_modules_and_output_format() {
        let s = setup_i3status(DEFAULT_I3STATUS_CONF);
        assert_eq!(s.schedule.slots.len(), 5);
        assert_eq!(s.interval, 1.0);
        assert_eq!(s.schedule.blocks[2].name.as_deref(), Some("disk"));
        assert_eq!(s.schedule.blocks[2].instance.as_deref(), Some("C:"));
        let s = setup_i3status(
            "general {\n output_format = \"none\"\n}\nbroken\norder += \"memory\"\n",
        );
        assert_eq!(s.output, OutputMode::Plain(" | ".into()));
        assert_eq!(s.interval, DEFAULT_INTERVAL);
        assert_eq!(s.schedule.blocks[0].full_text, "config error line 4");
        assert!(matches!(s.schedule.slots[1], Slot::Module(_)));
    }
}
