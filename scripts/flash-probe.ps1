# Measures visual gaps ("flash") during workspace and tab switches: a Notepad (or an
# Explorer window on an empty folder, -app explorer) fills the screen on each workspace /
# tab; a few screen points are read before each switch and then sampled as fast as
# possible for 600 ms after it. A point turning to the wallpaper colour (read with
# everything minimised) means the background showed through; other changes are
# fades or unpainted content.
# Commands go straight over the IPC pipe: a spawned i3win-msg console would itself show up
# as a flash (and get tiled), and process start-up would delay the sampling.
# Run: cargo build --workspace; powershell -ExecutionPolicy Bypass -File scripts\flash-probe.ps1 [-bin <dir>] [-app notepad|explorer] [-config <file>]
param([string]$bin = "$(Split-Path -Parent $PSScriptRoot)\target\debug", [string]$app = "notepad", [string]$config)
$ErrorActionPreference = 'Continue'
$exe = "$bin\i3win.exe"
Add-Type @"
using System; using System.Runtime.InteropServices; using System.Diagnostics; using System.IO.Pipes; using System.Text;
public class P {
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll")] public static extern uint GetPixel(IntPtr dc, int x, int y);
  // Sends one i3 IPC RUN_COMMAND straight over the pipe and waits for the reply, so
  // sampling can start the moment the switch has been applied.
  public static void Cmd(string payload) {
    using (var p = new NamedPipeClientStream(".", "i3win-ipc", PipeDirection.InOut)) {
      p.Connect(2000);
      byte[] body = Encoding.UTF8.GetBytes(payload);
      var m = new System.IO.MemoryStream();
      m.Write(Encoding.ASCII.GetBytes("i3-ipc"), 0, 6);
      m.Write(BitConverter.GetBytes(body.Length), 0, 4);
      m.Write(BitConverter.GetBytes(0), 0, 4);
      m.Write(body, 0, body.Length);
      byte[] msg = m.ToArray(); p.Write(msg, 0, msg.Length); p.Flush();
      byte[] hdr = new byte[14]; int got = 0;
      while (got < 14) { int k = p.Read(hdr, got, 14 - got); if (k <= 0) return; got += k; }
      int len = BitConverter.ToInt32(hdr, 6); byte[] rest = new byte[len]; got = 0;
      while (got < len) { int k = p.Read(rest, got, len - got); if (k <= 0) return; got += k; }
    }
  }
  public static uint Pixel(int x, int y) {
    IntPtr dc = GetDC(IntPtr.Zero); uint c = GetPixel(dc, x, y); ReleaseDC(IntPtr.Zero, dc); return c;
  }
  static int Ch(uint c, int s) { return (int)((c >> s) & 0xFF); }
  static bool Near(uint a, uint b) {
    return Math.Abs(Ch(a, 0) - Ch(b, 0)) <= 20 && Math.Abs(Ch(a, 8) - Ch(b, 8)) <= 20 && Math.Abs(Ch(a, 16) - Ch(b, 16)) <= 20;
  }
  // Samples the points for ms milliseconds. A sample is "diff" when any point differs
  // from its colour before the switch (want), and "wall" when a point that was not
  // showing the wallpaper before the switch now does (points where the wallpaper is
  // near-white are skipped: unpainted windows flash white too). Returns
  // "samples diff wall firstWallMs".
  public static string Sample(int[] xs, int[] ys, int ms, uint[] want, uint[] wall) {
    IntPtr dc = GetDC(IntPtr.Zero); var sw = Stopwatch.StartNew(); int n = 0, diff = 0, walls = 0; long first = -1; string seen = "";
    while (sw.ElapsedMilliseconds < ms) {
      bool d = false, w = false; n++;
      for (int i = 0; i < xs.Length; i++) {
        uint c = GetPixel(dc, xs[i], ys[i]);
        if (!Near(c, want[i])) d = true;
        if (Near(c, wall[i]) && !Near(want[i], wall[i]) && !Near(wall[i], 0xFFFFFF)) { w = true; if (seen == "") seen = "p" + i + "=" + c.ToString("X6") + " want " + want[i].ToString("X6"); }
      }
      if (d) diff++;
      if (w) { walls++; if (first < 0) first = sw.ElapsedMilliseconds; }
    }
    ReleaseDC(IntPtr.Zero, dc); return n + " " + diff + " " + walls + " " + first + " " + (seen == "" ? "-" : seen.Replace(" ", "_"));
  }
  public static uint[] Pixels(int[] xs, int[] ys) {
    var r = new uint[xs.Length]; for (int i = 0; i < xs.Length; i++) r[i] = Pixel(xs[i], ys[i]); return r;
  }
}
"@
function Msg { [P]::Cmd(($args -join ' ')) }
# Sample points spread over the screen (the tab bar sits near the top).
[int[]]$xs = 960, 480, 1440, 960, 300
[int[]]$ys = 540, 300, 800, 30, 1000
function Probe($label, [scriptblock]$switch) {
  $diffs = @(); $walls = @()
  for ($i = 0; $i -lt 6; $i++) {
    $want = [P]::Pixels($xs, $ys)
    & $switch $i
    $r = [P]::Sample($xs, $ys, 600, $want, $wall) -split ' '
    $diffs += [int]$r[1]; $walls += [int]$r[2]; if ($r[4] -ne '-') { "    wallpaper hit: $($r[4]) at $($r[3])ms" }
    Start-Sleep -Milliseconds 300
  }
  "{0}: wallpaper seen on {1}/6 switches (wallpaper samples: {2}; any-change samples: {3})" -f $label,
    @($walls | Where-Object { $_ -gt 0 }).Count, ($walls -join ','), ($diffs -join ',')
}

$running = Get-Process i3win -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty Path
$tmpCfg = -not $config
if ($tmpCfg) {
  $config = "$env:TEMP\i3win-flash.conf"
  Set-Content $config -Encoding ascii -Value "set `$mod Mod1`ndefault_border none`ngaps inner 0`n"
}
Get-Process i3win, i3win-status -ErrorAction SilentlyContinue | Stop-Process -Force
(New-Object -ComObject Shell.Application).MinimizeAll(); Start-Sleep 2
$wall = [P]::Pixels($xs, $ys)
"wallpaper colours: " + (($wall | ForEach-Object { $_.ToString('X6') }) -join ' ')
$wm = Start-Process $exe -ArgumentList '--config', $config -PassThru; Start-Sleep 1.5
$np = @(); $dirs = @()
# Opens one window of the chosen app; an Explorer window gets its own empty folder.
function Open-App {
  if ($app -eq 'explorer') {
    $d = "$env:TEMP\i3win-flash-$($dirs.Count)"; New-Item -ItemType Directory -Force $d | Out-Null
    $script:dirs += $d; Start-Process explorer.exe $d; Start-Sleep 2.5
  } else { $script:np += Start-Process notepad -PassThru; Start-Sleep 1.5 }
}
try {
  # Workspace switch: one full-screen window on ws 1 and ws 2.
  Open-App
  Msg workspace 2; Start-Sleep 0.4
  Open-App
  Probe 'workspace switch' { param($i) if ($i % 2 -eq 0) { Msg workspace 1 } else { Msg workspace 2 } }

  # Tab switch: two windows in a tabbed container on ws 3.
  Msg workspace 3; Start-Sleep 0.4
  Open-App
  Open-App
  Msg layout tabbed; Start-Sleep 0.4
  Probe 'tab switch' { param($i) if ($i % 2 -eq 0) { Msg focus left } else { Msg focus right } }

  Msg layout stacking; Start-Sleep 0.4
  Probe 'stack switch' { param($i) if ($i % 2 -eq 0) { Msg focus up } else { Msg focus down } }
}
finally {
  Msg exit; Start-Sleep 1
  Get-Process i3win, i3win-status -ErrorAction SilentlyContinue | Stop-Process -Force
  $np | Stop-Process -Force -ErrorAction SilentlyContinue
  (New-Object -ComObject Shell.Application).Windows() | Where-Object { $_.LocationURL -like '*i3win-flash-*' } | ForEach-Object { $_.Quit() }
  Start-Sleep 0.5; $dirs | Remove-Item -Force -ErrorAction SilentlyContinue
  if ($tmpCfg) { Remove-Item $config -ErrorAction SilentlyContinue }
  # Restart an i3win that was running before the probe (it restores its hidden windows).
  if ($running) { Start-Process $running; Start-Sleep 2 }
  (New-Object -ComObject Shell.Application).UndoMinimizeALL()
}
