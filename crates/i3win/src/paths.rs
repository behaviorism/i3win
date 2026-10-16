use std::path::{Path, PathBuf};

fn env_dir(var: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Config path: the CLI override, else `%USERPROFILE%\.config\i3win\config`.
pub fn config_path(cli_override: Option<PathBuf>) -> PathBuf {
    cli_override.unwrap_or_else(|| {
        env_dir("USERPROFILE")
            .join(".config")
            .join("i3win")
            .join("config")
    })
}

/// `%LOCALAPPDATA%\i3win`, created if missing.
pub fn data_dir() -> PathBuf {
    let dir = env_dir("LOCALAPPDATA").join("i3win");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn log_path() -> PathBuf {
    data_dir().join("i3win.log")
}

pub fn hidden_path() -> PathBuf {
    data_dir().join("hidden.txt")
}

/// Pure: true if `dir\exe` exists for any `;`-separated dir in `path_var`.
pub fn find_on_path(exe: &str, path_var: &str) -> bool {
    path_var
        .split(';')
        .filter(|d| !d.is_empty())
        .any(|d| Path::new(d).join(exe).exists())
}

/// Reads the config, first writing the default one if the file is missing.
pub fn load_or_create_config(path: &Path) -> Result<String, String> {
    if !path.exists() {
        let path_var = std::env::var("PATH").unwrap_or_default();
        let text = i3win_core::config::default_config_text(find_on_path("wt.exe", &path_var));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        std::fs::write(path, &text).map_err(|e| format!("write {}: {e}", path.display()))?;
        return Ok(text);
    }
    std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_on_path_checks_each_dir() {
        let dir = std::env::temp_dir().join("i3win_test_path");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fake.exe"), b"").unwrap();
        let pv = format!("C:\\nonexistent;{}", dir.display());
        assert!(find_on_path("fake.exe", &pv));
        assert!(!find_on_path("other.exe", &pv));
    }

    #[test]
    fn creates_default_config_when_missing() {
        let p = std::env::temp_dir().join("i3win_test_cfg").join("config");
        let _ = std::fs::remove_file(&p);
        let text = load_or_create_config(&p).unwrap();
        assert!(p.exists() && text.contains("bindsym $mod+Return"));
        assert_eq!(load_or_create_config(&p).unwrap(), text);
    }
}
