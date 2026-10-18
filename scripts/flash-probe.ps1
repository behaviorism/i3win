# Measures visual gaps ("flash") during workspace and tab switches: a Notepad (or an
# Explorer window on an empty folder, -app explorer) fills the screen on each workspace /
# tab; the pixel at the screen centre is read before each switch and then sampled as fast
# as possible for 600 ms after it. Any sample that differs from the colour before the
# switch means the wallpaper, a fade or unpainted content showed through.
# Commands go straight over the IPC pipe: a spawned i3win-msg console would itself show up
# as a flash (and get tiled), and process start-up would delay the sampling.
# Run: cargo build --workspace; powershell -ExecutionPolicy Bypass -File scripts\flash-probe.ps1 [-bin <dir>] [-app notepad|explorer]
param([string]$bin = "$(Split-Path -Parent $PSScriptRoot)\target\debug", [string]$app = "notepad")
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
  // Samples (x,y) for ms milliseconds, counting samples that differ from want;
  // returns "samples bad firstBadMs lastBadColour".
  public static string Sample(int x, int y, int ms, uint want) {
    IntPtr dc = GetDC(IntPtr.Zero); var sw = Stopwatch.StartNew(); int n = 0, bad = 0; long first = -1; uint last = 0;
    while (sw.ElapsedMilliseconds < ms) {
      uint c = GetPixel(dc, x, y); n++;
      if (Math.Abs(Ch(c, 0) - Ch(want, 0)) > 20 || Math.Abs(Ch(c, 8) - Ch(want, 8)) > 20 || Math.Abs(Ch(c, 16) - Ch(want, 16)) > 20) {
        bad++; if (first < 0) first = sw.ElapsedMilliseconds; last = c;
      }
    }
    ReleaseDC(IntPtr.Zero, dc); return n + " " + bad + " " + first + " " + last.ToString("X6");
  }
}
"@
function Msg { [P]::Cmd(($args -join ' ')) }
function Probe($label, [scriptblock]$switch) {
  $results = @(); $detail = @()
  for ($i = 0; $i -lt 6; $i++) {
    $want = [P]::Pixel(960, 540)
    & $switch $i
    $r = [P]::Sample(960, 540, 600, $want) -split ' '
    $results += [int]$r[1]; $detail += "n=$($r[0]) bad=$($r[1]) at=$($r[2])ms col=$($r[3]) want=$($want.ToString('X6'))"
    Start-Sleep -Milliseconds 300
  }
  $flashes = @($results | Where-Object { $_ -gt 0 }).Count
  "{0}: {1}/6 switches showed a different colour (bad samples per switch: {2})" -f $label, $flashes, ($results -join ',')
  $detail | ForEach-Object { "    $_" }
}

$cfg = "$env:TEMP\i3win-flash.conf"
Set-Content $cfg -Encoding ascii -Value "set `$mod Mod1`ndefault_border none`ngaps inner 0`n"
(New-Object -ComObject Shell.Application).MinimizeAll(); Start-Sleep 2
Get-Process i3win, i3win-status -ErrorAction SilentlyContinue | Stop-Process -Force
$wm = Start-Process $exe -ArgumentList '--config', $cfg -PassThru; Start-Sleep 1.5
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
  Remove-Item $cfg -ErrorAction SilentlyContinue
  (New-Object -ComObject Shell.Application).UndoMinimizeALL()
}
