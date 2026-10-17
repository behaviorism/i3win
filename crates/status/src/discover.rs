//! Config format detection and config file discovery. Pure; no I/O of its own.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    I3Status,
    I3Blocks,
}

/// First non-comment, non-blank line: `[section]` or a `key=value` without
/// `{` means i3blocks; anything else is i3status.
pub fn detect_format(text: &str) -> Format {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') || (line.contains('=') && !line.contains('{')) {
            return Format::I3Blocks;
        }
        return Format::I3Status;
    }
    Format::I3Status
}

/// Picks the config path. Returns `Some(format)` when the location fixes it;
/// for an explicit `-c PATH` returns `None` (caller detects from content).
/// When nothing exists, returns the default i3status path to be written.
pub fn discover(
    cli: Option<&Path>,
    env_i3blocks: Option<&str>,
    home: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> (PathBuf, Option<Format>) {
    if let Some(p) = cli {
        return (p.to_path_buf(), None);
    }
    if let Some(e) = env_i3blocks {
        return (PathBuf::from(e), Some(Format::I3Blocks));
    }
    let cfg = home.join(".config");
    let candidates = [
        (cfg.join("i3win").join("i3blocks.conf"), Format::I3Blocks),
        (cfg.join("i3blocks").join("config"), Format::I3Blocks),
        (home.join(".i3blocks.conf"), Format::I3Blocks),
        (cfg.join("i3win").join("i3status.conf"), Format::I3Status),
    ];
    for (p, f) in &candidates {
        if exists(p) {
            return (p.clone(), Some(*f));
        }
    }
    (
        cfg.join("i3win").join("i3status.conf"),
        Some(Format::I3Status),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_and_discover() {
        assert_eq!(detect_format("# c\n[cpu]\n"), Format::I3Blocks);
        assert_eq!(detect_format("interval=5\n"), Format::I3Blocks);
        assert_eq!(detect_format("general {\n}\n"), Format::I3Status);
        let home = Path::new("H");
        let only = |p: &'static str| move |q: &Path| q == Path::new(p);
        assert_eq!(
            discover(None, None, home, &only(r"H\.config\i3blocks\config")),
            (
                PathBuf::from(r"H\.config\i3blocks\config"),
                Some(Format::I3Blocks)
            )
        );
        assert_eq!(
            discover(None, Some("E"), home, &|_| true),
            (PathBuf::from("E"), Some(Format::I3Blocks))
        );
        assert_eq!(
            discover(Some(Path::new("X")), Some("E"), home, &|_| true),
            (PathBuf::from("X"), None)
        );
        assert_eq!(
            discover(None, None, home, &|_| false),
            (
                PathBuf::from(r"H\.config\i3win\i3status.conf"),
                Some(Format::I3Status)
            )
        );
        assert_eq!(
            discover(None, None, home, &only(r"H\.config\i3win\i3blocks.conf")).1,
            Some(Format::I3Blocks)
        );
    }

    #[test]
    fn i3blocks_wins_over_i3status_and_dotfile() {
        let home = Path::new("H");
        let both = |q: &Path| {
            q == Path::new(r"H\.config\i3win\i3blocks.conf")
                || q == Path::new(r"H\.config\i3win\i3status.conf")
        };
        assert_eq!(
            discover(None, None, home, &both),
            (
                PathBuf::from(r"H\.config\i3win\i3blocks.conf"),
                Some(Format::I3Blocks)
            )
        );
        let dot = |q: &Path| q == Path::new(r"H\.i3blocks.conf");
        assert_eq!(
            discover(None, None, home, &dot),
            (PathBuf::from(r"H\.i3blocks.conf"), Some(Format::I3Blocks))
        );
        let status = |q: &Path| q == Path::new(r"H\.config\i3win\i3status.conf");
        assert_eq!(
            discover(None, None, home, &status).1,
            Some(Format::I3Status)
        );
    }

    #[test]
    fn detect_ignores_bom() {
        assert_eq!(detect_format("\u{feff}[cpu]\n"), Format::I3Blocks);
        assert_eq!(detect_format("\u{feff}general {\n}\n"), Format::I3Status);
    }
}
