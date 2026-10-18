# Releasing i3win

One-time setup: install Inno Setup 6 (`winget install JRSoftware.InnoSetup`).

For each release:

1. Bump `version` in the four `crates/*/Cargo.toml` files (they share one version; `i3win --version` and IPC `get_version` read it).
2. Run `powershell -ExecutionPolicy Bypass -File scripts\release.ps1`. It builds the release binaries and writes `dist\i3win-setup-<version>.exe`.
3. Commit, tag (`git tag v<version>`), and attach the setup exe to a GitHub Release if you publish one.

The installer (`installer\i3win.iss`) installs per user into `%LOCALAPPDATA%\Programs\i3win` (no admin), adds Start-menu shortcuts ("i3win" and "Exit i3win"), and offers "Start i3win when I sign in" (HKCU Run key). Installing over an older version, or uninstalling, first asks a running i3win to exit cleanly so every hidden window comes back; the `AppId` in the script must never change, or upgrades will install side by side.
