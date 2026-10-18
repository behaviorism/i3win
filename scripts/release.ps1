# Builds a release of i3win: release binaries, then dist\i3win-setup-<version>.exe.
# The version comes from crates\i3win\Cargo.toml. Requires Inno Setup 6
# (winget install JRSoftware.InnoSetup).
# Run: powershell -ExecutionPolicy Bypass -File scripts\release.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path 'crates\i3win\Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
Write-Host "i3win $version"

cargo build --release --workspace
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

$iscc = @(
  "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
  "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
  "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 not found: winget install JRSoftware.InnoSetup' }

& $iscc /Q "/DAppVersion=$version" 'installer\i3win.iss'
if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }

$out = Join-Path $root "dist\i3win-setup-$version.exe"
Write-Host "Built $out ($([Math]::Round((Get-Item $out).Length / 1MB, 1)) MB)"
