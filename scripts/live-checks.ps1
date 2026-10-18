# Live desktop checks for i3win: drives the WM with synthetic keystrokes and i3win-msg,
# verifying results through Win32. Uses your ~/.config/i3win/config (Alt as $mod, default bindings).
# It minimizes every window, opens and kills Notepads, and kills any running i3win.
# Run: cargo build --workspace; powershell -ExecutionPolicy Bypass -File scripts\live-checks.ps1
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$exe = "$root\target\debug\i3win.exe"
$msg = "$root\target\debug\i3win-msg.exe"
$scratch = $env:TEMP
$results = New-Object System.Collections.ArrayList

Add-Type @"
using System; using System.Text; using System.Runtime.InteropServices; using System.Collections.Generic;
public class U {
  public delegate bool P(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(P p, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int size);
  // On screen: visible and not cloaked (i3win hides by cloaking by default).
  public static bool Shown(IntPtr h) { int c; DwmGetWindowAttribute(h, 14, out c, 4); return IsWindowVisible(h) && c == 0; }
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint c);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string c, string w);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint c, uint t);
  [DllImport("user32.dll")] public static extern bool SystemParametersInfo(uint a, uint b, out RECT r, uint c);
  public static RECT WorkArea(){ RECT r; SystemParametersInfo(0x30,0,out r,0); return r; }
  public struct RECT { public int L,T,R,B; public override string ToString(){ return "["+L+","+T+","+R+","+B+"]"; } }
  public static IntPtr Top(uint pid) {
    IntPtr f = IntPtr.Zero;
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h,out p); if(p==pid && GetWindow(h,4)==IntPtr.Zero){ var s=new StringBuilder(64); GetClassName(h,s,64); if(s.ToString()=="Notepad"){ f=h; return false; } } return true; }, IntPtr.Zero);
    return f;
  }
  public static List<string> OwnWindows(uint pid) {
    var o = new List<string>();
    EnumWindows((h,l)=>{ uint p; GetWindowThreadProcessId(h,out p); if(p==pid && IsWindowVisible(h)){ var s=new StringBuilder(64); GetClassName(h,s,64); RECT r; GetWindowRect(h,out r); o.Add(s+" "+r); } return true; }, IntPtr.Zero);
    return o;
  }
  public static string EditText(IntPtr top) {
    var e = FindWindowEx(top, IntPtr.Zero, "Edit", null);
    var s = new StringBuilder(4096); SendMessage(e, 0x0D, (IntPtr)4096, s); return s.ToString();
  }
  public static string Cls(IntPtr h){ var s=new StringBuilder(128); GetClassName(h,s,128); return s.ToString(); }
  public static RECT Rect(IntPtr h){ RECT r; GetWindowRect(h,out r); return r; }
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int dx, int dy, uint d, UIntPtr extra);
  public static void Mouse(uint flags){ mouse_event(flags, 0, 0, 0, UIntPtr.Zero); }
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  // Real mouse-move input (SetCursorPos alone is invisible to low-level mouse hooks).
  public static void MoveTo(int x, int y){ SetCursorPos(x, y); int w = GetSystemMetrics(0), h = GetSystemMetrics(1); mouse_event(0x8001, (int)((long)x * 65535 / (w - 1)), (int)((long)y * 65535 / (h - 1)), 0, UIntPtr.Zero); }
  public static void Key(byte vk, bool up){ keybd_event(vk,(byte)MapVirtualKey(vk,0), up?2u:0u, UIntPtr.Zero); }
}
"@

function Check($name, $ok, $detail = '') {
  $s = if ($ok) { 'PASS' } else { 'FAIL' }
  $line = "$s  $name  $detail"
  [void]$results.Add($line); Write-Output $line
}
function Combo([byte[]]$mods, [byte]$vk) {
  foreach ($m in $mods) { [U]::Key($m, $false); Start-Sleep -Milliseconds 20 }
  [U]::Key($vk, $false); Start-Sleep -Milliseconds 20; [U]::Key($vk, $true)
  Start-Sleep -Milliseconds 20
  [array]::Reverse($mods); foreach ($m in $mods) { [U]::Key($m, $true); Start-Sleep -Milliseconds 20 }
  Start-Sleep -Milliseconds 350
}
function Type-Text($t) { foreach ($c in $t.ToUpper().ToCharArray()) { [U]::Key([byte][char]$c, $false); [U]::Key([byte][char]$c, $true); Start-Sleep -Milliseconds 30 }; Start-Sleep -Milliseconds 300 }
$ALT = [byte]0xA4; $SHIFT = [byte]0xA0
function Fg { [U]::GetForegroundWindow() }
function Msg { $o = & $msg @args 2>&1 | Out-String; return @{ out = $o.Trim(); code = $LASTEXITCODE } }
function StatusCount { @(Get-Process i3win-status -ErrorAction SilentlyContinue).Count }
function Start-Wm { $p = Start-Process $exe -PassThru; Start-Sleep -Milliseconds 1500; return $p }

# ---- start
(New-Object -ComObject Shell.Application).MinimizeAll(); Start-Sleep 2
Get-Process i3win, i3win-status -ErrorAction SilentlyContinue | Stop-Process -Force
$wm = Start-Wm
Check 'i3win starts' (-not $wm.HasExited)
$bars = [U]::OwnWindows($wm.Id)
Write-Output "i3win windows: $($bars -join ' | ')"
Check 'one status process' ((StatusCount) -eq 1) "count=$(StatusCount)"

# ---- 3 notepads tile
$np = @(); $h = @()
for ($i = 0; $i -lt 3; $i++) {
  $p = Start-Process notepad -PassThru; $np += $p
  $t = [IntPtr]::Zero; for ($k = 0; $k -lt 50 -and $t -eq [IntPtr]::Zero; $k++) { Start-Sleep -Milliseconds 100; $t = [U]::Top($p.Id) }
  $h += $t
}
Start-Sleep 1
$rects = $h | ForEach-Object { [U]::Rect($_) }
Write-Output "rects: $($rects -join ' ')"
$sorted = $rects | Sort-Object L
$widths = $sorted | ForEach-Object { $_.R - $_.L }
Check '3 notepads tile in 3 columns' (($sorted[0].R -le $sorted[1].L + 16) -and ($sorted[1].R -le $sorted[2].L + 16) -and (($widths | Measure-Object -Max).Maximum - ($widths | Measure-Object -Min).Minimum -le 4)) "widths=$($widths -join ',')"
Check 'tiles share top/bottom' ((($rects | ForEach-Object T | Sort-Object -Unique).Count -eq 1) -and (($rects | ForEach-Object B | Sort-Object -Unique).Count -eq 1))
$focusedIdx = [array]::IndexOf($h, (Fg))
Check 'newest notepad focused' ((Fg) -eq $h[2]) "fg=$(Fg) cls=$([U]::Cls((Fg)))"

# ---- focus follows keyboard
Combo @($ALT) 0x4A   # Alt+j focus left
$byL = $h | Sort-Object { [U]::Rect($_).L }
Check 'Alt+j moves focus to middle column' ((Fg) -eq $byL[1]) "fg=$(Fg) cls=$([U]::Cls((Fg)))"
Type-Text 'abc'
Check 'typing lands in focused notepad' ([U]::EditText($byL[1]) -match 'abc') "text='$([U]::EditText($byL[1]))'"
for ($i = 0; $i -lt 10; $i++) { Combo @($ALT) 0x4A }
$fgAfter = Fg
Check '10x Alt+j: focus still on a notepad' ($h -contains $fgAfter) "fg=$fgAfter cls=$([U]::Cls($fgAfter))"
Combo @($ALT) 0xBA   # Alt+; focus right
Check 'Alt+; focus right' ((Fg) -ne $fgAfter -and $h -contains (Fg))
Check 'no menu/start after Alt' (@('Notepad') -contains [U]::Cls((Fg))) "cls=$([U]::Cls((Fg)))"

# ---- workspaces
$before = $h | ForEach-Object { [U]::Rect($_).ToString() }
Combo @($ALT) 0x32   # Alt+2
$vis = $h | Where-Object { [U]::Shown($_) }
Check 'Alt+2 hides notepads' (@($vis).Count -eq 0) "visible=$(@($vis).Count)"
Type-Text 'zzz'
Combo @($ALT) 0x31   # Alt+1
Start-Sleep -Milliseconds 300
$vis = $h | Where-Object { [U]::Shown($_) }
$after = $h | ForEach-Object { [U]::Rect($_).ToString() }
Check 'Alt+1 shows them again' (@($vis).Count -eq 3)
Check 'layout unchanged after round trip' ((Compare-Object $before $after) -eq $null) "before=$before after=$after"
$leak = $h | Where-Object { [U]::EditText($_) -match 'zzz' }
Check 'typing on empty ws2 did not reach hidden notepad' (@($leak).Count -eq 0)
Check 'focus on a notepad after return' ($h -contains (Fg)) "cls=$([U]::Cls((Fg)))"

# ---- layouts
Combo @($ALT) 0x57   # Alt+w tabbed
$vis = @($h | Where-Object { [U]::Shown($_) })
Write-Output "tabbed visible=$($vis.Count) rects: $(($h | % { [U]::Rect($_) }) -join ' ')"
Check 'tabbed shows one window' ($vis.Count -eq 1)
Combo @($ALT) 0x53   # Alt+s stacking
$vis = @($h | Where-Object { [U]::Shown($_) })
Check 'stacked shows one window' ($vis.Count -eq 1)
Combo @($ALT) 0x45   # Alt+e toggle split
Start-Sleep -Milliseconds 300
$vis = @($h | Where-Object { [U]::Shown($_) })
Check 'split shows all three' ($vis.Count -eq 3) "rects: $(($h | % { [U]::Rect($_) }) -join ' ')"
$f = Fg; $r0 = [U]::Rect($f)
Combo @($ALT, $SHIFT) 0x20   # floating toggle
$r1 = [U]::Rect($f)
Check 'Alt+Shift+space floats window' ($r0.ToString() -ne $r1.ToString()) "$r0 -> $r1"
Combo @($ALT, $SHIFT) 0x20
Start-Sleep -Milliseconds 300
Check 'toggle back re-tiles' (([U]::Rect($f)).ToString() -eq $r0.ToString()) "$r0 vs $([U]::Rect($f))"

# ---- IPC
foreach ($t in 'get_workspaces','get_outputs','get_tree','get_version','get_binding_modes','get_config','get_bar_config','get_marks') {
  $r = Msg -t $t
  $okJson = $true; try { $null = $r.out | ConvertFrom-Json } catch { $okJson = $false }
  Check "ipc $t valid JSON" ($okJson -and $r.code -eq 0) ($r.out.Substring(0, [Math]::Min(80, $r.out.Length)))
}
$sub = Start-Process $msg -ArgumentList '-t subscribe -m "[\"window\",\"workspace\",\"tick\"]"' -PassThru -NoNewWindow -RedirectStandardOutput "$scratch\sub.txt"
Start-Sleep -Milliseconds 700
$r = Msg workspace 4
Check 'ipc run_command workspace 4' ($r.out -match '"success":true') $r.out
$r = Msg frob
Check 'ipc bad command exits 2' ($r.code -eq 2) "code=$($r.code) $($r.out)"
$p4 = Start-Process notepad -PassThru; Start-Sleep 1.5
$h4 = [U]::Top($p4.Id)
Combo @($ALT, $SHIFT) 0x51   # kill
Start-Sleep 1
Check 'Alt+Shift+q closes notepad' ($p4.HasExited)
$null = Msg -t send_tick hello
Start-Sleep -Milliseconds 500
$null = Msg workspace 1
Start-Sleep -Milliseconds 500
$sub | Stop-Process -Force -ErrorAction SilentlyContinue
$ev = Get-Content "$scratch\sub.txt" -Raw
$iNew = $ev.IndexOf('"change":"new"'); $iFoc = $ev.IndexOf('"change":"focus"', [Math]::Max(0,$iNew))
Check 'events: window new then focus' ($iNew -ge 0 -and $iFoc -gt $iNew)
Check 'events: window close' ($ev -match '"change":"close"')
Check 'events: workspace init/focus' ($ev -match '"change":"init"' -and $ev -match '"change":"focus"')
Check 'events: tick hello' ($ev -match '"payload":"hello"')
Write-Output "events (head): $($ev.Substring(0,[Math]::Min(600,$ev.Length)))"

# ---- reload / status process
for ($i = 0; $i -lt 5; $i++) { Combo @($ALT, $SHIFT) 0x43; Start-Sleep -Milliseconds 300 }
Start-Sleep 1
Check '5 reloads leave one status process' ((StatusCount) -eq 1) "count=$(StatusCount)"
Check 'still running after reloads' (-not $wm.HasExited)

# ---- Plan 5: marks, scratchpad, rename, gaps (placed while the WM is still running; state is restored)
$f = Fg
$r = Msg mark x
Check 'plan5 mark x' ($r.out -match '"success":true') $r.out
$r = Msg -t get_marks
Check 'plan5 get_marks contains "x"' ($r.out -match '"x"') $r.out
$r = Msg move scratchpad
Start-Sleep -Milliseconds 500
Check 'plan5 move scratchpad hides focused notepad' (-not [U]::Shown($f)) "visible=$([U]::Shown($f))"
$r = Msg scratchpad show
Start-Sleep -Milliseconds 500
Check 'plan5 scratchpad show shows it' ([U]::Shown($f)) "visible=$([U]::Shown($f))"
$null = Msg floating disable
$null = Msg unmark x
Start-Sleep -Milliseconds 400
$r = Msg rename workspace to 9
Check 'plan5 rename workspace to 9' ($r.out -match '"success":true') $r.out
$r = Msg -t get_workspaces
Check 'plan5 get_workspaces contains "9"' ($r.out -match '"9"') $r.out
$null = Msg rename workspace to 1
Start-Sleep -Milliseconds 300
$rb = $h | ForEach-Object { [U]::Rect($_).ToString() }
$r = Msg gaps inner current set 20
Start-Sleep -Milliseconds 500
$ra = $h | ForEach-Object { [U]::Rect($_).ToString() }
Check 'plan5 gaps inner current set 20 changes rects' ($null -ne (Compare-Object $rb $ra)) "before=$rb after=$ra"
$null = Msg gaps inner current set 0
Start-Sleep -Milliseconds 400

# ---- Plan 6: bar (default config, one bar)
$bar = [U]::FindWindowEx([IntPtr]::Zero, [IntPtr]::Zero, 'i3win_bar', $null)
Check 'plan6 bar window exists' ($bar -ne [IntPtr]::Zero)
$wa = [U]::WorkArea()
$br = [U]::Rect($bar)
Check 'plan6 bar sits at the bottom of the work area' ([U]::Shown($bar) -and $br.B -eq $wa.B -and $br.B -gt $br.T) "bar=$br workarea=$wa"
$r = Msg -t get_bar_config
Check 'plan6 get_bar_config lists bar-0' ($r.out -match '^\[\s*"bar-0"\s*\]$') $r.out
$hb = [U]::Rect($h[0]); $heightBefore = $hb.B - $hb.T
$null = Msg bar mode hide
Start-Sleep -Milliseconds 700
$bar = [U]::FindWindowEx([IntPtr]::Zero, [IntPtr]::Zero, 'i3win_bar', $null)
Check 'plan6 bar mode hide hides the bar' (($bar -eq [IntPtr]::Zero) -or -not [U]::Shown($bar))
$hb = [U]::Rect($h[0])
Check 'plan6 bar mode hide grows the Notepads' (($hb.B - $hb.T) -gt $heightBefore) "before=$heightBefore after=$($hb.B - $hb.T)"
$null = Msg bar mode dock
Start-Sleep -Milliseconds 700
$bar = [U]::FindWindowEx([IntPtr]::Zero, [IntPtr]::Zero, 'i3win_bar', $null)
Check 'plan6 bar mode dock restores the bar' (($bar -ne [IntPtr]::Zero) -and [U]::Shown($bar))
$hb = [U]::Rect($h[0])
Check 'plan6 bar mode dock restores the Notepad height' (($hb.B - $hb.T) -eq $heightBefore) "before=$heightBefore after=$($hb.B - $hb.T)"

# ---- crash recovery: taskkill on ws2
Combo @($ALT) 0x32
$vis = @($h | Where-Object { [U]::Shown($_) })
Check 'ws2 hides before kill' ($vis.Count -eq 0)
& taskkill /IM i3win.exe /F | Out-Null
Start-Sleep 1
$vis = @($h | Where-Object { [U]::Shown($_) })
Check 'killed: notepads stay hidden' ($vis.Count -eq 0)
Check 'killed: status process gone (job object)' ((StatusCount) -eq 0) "count=$(StatusCount)"
$hiddenTxt = Get-Content "$env:LOCALAPPDATA\i3win\hidden.txt" -ErrorAction SilentlyContinue
Check 'hidden.txt lists 3 windows' (@($hiddenTxt).Count -eq 3) ($hiddenTxt -join ';')
$wm = Start-Wm
Start-Sleep 1
$vis = @($h | Where-Object { [U]::Shown($_) })
Check 'restart: notepads reappear' ($vis.Count -eq 3)
Write-Output "after restart rects: $(($h | % { [U]::Rect($_) }) -join ' ')"

# ---- exit restores
Combo @($ALT) 0x32
Combo @($ALT, $SHIFT) 0x45   # exit
Start-Sleep 1.5
Check 'Alt+Shift+e exits' ($wm.HasExited)
$vis = @($h | Where-Object { [U]::Shown($_) })
Check 'exit: all windows visible' ($vis.Count -eq 3)
Check 'exit: no status process' ((StatusCount) -eq 0)

# ---- i3win-status standalone (i3blocks command block)
$statusExe = "$root\target\debug\i3win-status.exe"
$blocksConf = "$scratch\i3win-live-i3blocks.conf"
[IO.File]::WriteAllText($blocksConf, "[t]`ncommand=echo hi`ninterval=1`n")
# i3win-status quits on stdin EOF, so give it its own stdin pipe and keep it
# open until the end (independent of this script's own stdin).
$psi = New-Object System.Diagnostics.ProcessStartInfo $statusExe
$psi.Arguments = "-c `"$blocksConf`""
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$sp = [System.Diagnostics.Process]::Start($psi)
$readTask = $sp.StandardOutput.ReadToEndAsync()
Start-Sleep -Milliseconds 2500
Check 'status: still running with stdin open' (-not $sp.HasExited)
Stop-Process -Id $sp.Id -Force -ErrorAction SilentlyContinue
$sout = if ($readTask.Wait(3000)) { $readTask.Result } else { '' }
$sp.StandardInput.Close()
Check 'status: i3bar header' ($sout -like '*{"version":1,"click_events":true}*')
Check 'status: command block output' ($sout -like '*"full_text":"hi"*')
Remove-Item $blocksConf -ErrorAction SilentlyContinue

# ---- Plan 8: mouse (runs i3win with a temp config: user's config + floating_modifier Mod1 + focus_follows_mouse yes)
# MOUSEEVENTF: LEFTDOWN 2, LEFTUP 4, RIGHTDOWN 8, RIGHTUP 0x10. Tolerance +-12 px (DWM frame differences).
# Held button/key are tracked so `finally` sends only the matching up events.
$script:heldUp = $null; $script:heldAlt = $false
function Mouse-Drag($x1, $y1, $x2, $y2, [uint32]$down, [uint32]$up, [bool]$withAlt = $true) {
  if ($withAlt) { [U]::Key($ALT, $false); $script:heldAlt = $true; Start-Sleep -Milliseconds 80 }
  [U]::MoveTo($x1, $y1); Start-Sleep -Milliseconds 100
  [U]::Mouse($down); $script:heldUp = $up; Start-Sleep -Milliseconds 100
  for ($s = 1; $s -le 10; $s++) {
    [U]::MoveTo([int]($x1 + ($x2 - $x1) * $s / 10), [int]($y1 + ($y2 - $y1) * $s / 10)); Start-Sleep -Milliseconds 50
  }
  Start-Sleep -Milliseconds 100
  [U]::Mouse($up); $script:heldUp = $null; Start-Sleep -Milliseconds 100
  if ($withAlt) { [U]::Key($ALT, $true); $script:heldAlt = $false }
  Start-Sleep -Milliseconds 500
}
# Focuses a known notepad through IPC instead of trusting the current foreground.
function Focus-Window($h) { $null = Msg "[id=$([int64]$h)] focus"; Start-Sleep -Milliseconds 400 }
function Near($a, $b, $tol = 12) { [Math]::Abs($a - $b) -le $tol }
$mp = @()
try {
  Get-Process i3win, i3win-status -ErrorAction SilentlyContinue | Stop-Process -Force
  $userCfg = "$env:USERPROFILE\.config\i3win\config"
  $base = if (Test-Path $userCfg) { [IO.File]::ReadAllText($userCfg) } else { '' }
  $tmpCfg = "$scratch\i3win-live-mouse.conf"
  [IO.File]::WriteAllText($tmpCfg, $base + "`nfloating_modifier Mod1`nfocus_follows_mouse yes`n")
  $wmM = Start-Process $exe -ArgumentList "--config `"$tmpCfg`"" -PassThru; Start-Sleep -Milliseconds 1500
  Check 'plan8 i3win starts with temp mouse config' (-not $wmM.HasExited)
  $null = Msg workspace 3; Start-Sleep -Milliseconds 400
  $mh = @()
  for ($i = 0; $i -lt 2; $i++) {
    $p = Start-Process notepad -PassThru; $mp += $p
    $t = [IntPtr]::Zero; for ($k = 0; $k -lt 50 -and $t -eq [IntPtr]::Zero; $k++) { Start-Sleep -Milliseconds 100; $t = [U]::Top($p.Id) }
    $mh += $t
  }
  Start-Sleep -Milliseconds 800
  $pair = @($mh | Sort-Object { [U]::Rect($_).L })
  $left = $pair[0]; $right = $pair[1]
  $rl = [U]::Rect($left); $rr = [U]::Rect($right)
  Write-Output "plan8 start rects: left=$rl right=$rr"
  Check 'plan8 two notepads tiled side by side' (($rl.R -le $rr.L + 16) -and ($rl.R -gt $rl.L)) "left=$rl right=$rr"

  # Alt + right-drag near left window's right edge resizes it
  Focus-Window $left
  $rl = [U]::Rect($left)
  $widthBefore = $rl.R - $rl.L
  $y = [int](($rl.T + $rl.B) / 2)
  Mouse-Drag ($rl.R - 8) $y ($rl.R + 92) $y 8 0x10
  $rl2 = [U]::Rect($left)
  Check 'plan8 Alt+right-drag resizes left tile ~100px wider' (Near ($rl2.R - $rl2.L) ($widthBefore + 100)) "before=$widthBefore after=$($rl2.R - $rl2.L)"

  # Alt + left-drag right tile onto the left tile's left quarter swaps them
  Focus-Window $right
  $rr = [U]::Rect($right); $rl2 = [U]::Rect($left)
  $wa = [U]::WorkArea()
  Mouse-Drag ([int](($rr.L + $rr.R) / 2)) ([int](($rr.T + $rr.B) / 2)) ($rl2.L + [int](($rl2.R - $rl2.L) / 4)) ([int](($rl2.T + $rl2.B) / 2)) 2 4
  $rr3 = [U]::Rect($right); $rl3 = [U]::Rect($left)
  Check 'plan8 Alt+left-drag swaps tiles (right notepad now at left edge)' (Near $rr3.L $wa.L 16) "right=$rr3 workarea=$wa"
  Check 'plan8 Alt+left-drag swaps tiles (left notepad now right of it)' ($rl3.L -ge $rr3.R - 16) "left=$rl3 right=$rr3"

  # floating window moves with Alt + left-drag
  Focus-Window $left
  $null = Msg "[id=$([int64]$left)] floating enable"; Start-Sleep -Milliseconds 500
  $f = $left
  $rf = [U]::Rect($f)
  Check 'plan8 floating enable on a known notepad' ((Fg) -eq $f) "fg=$(Fg) cls=$([U]::Cls((Fg))) expected=$f"
  Mouse-Drag ([int](($rf.L + $rf.R) / 2)) ([int](($rf.T + $rf.B) / 2)) ([int](($rf.L + $rf.R) / 2) + 100) ([int](($rf.T + $rf.B) / 2)) 2 4
  $rf2 = [U]::Rect($f)
  Check 'plan8 Alt+left-drag moves floating window ~100px' ((Near ($rf2.L - $rf.L) 100) -and (Near $rf2.T $rf.T)) "before=$rf after=$rf2"
  $null = Msg "[id=$([int64]$f)] floating disable"; Start-Sleep -Milliseconds 600

  # hover focus: focus one notepad, then move onto the other
  Focus-Window $left
  $other = $right
  $ro = [U]::Rect($other)
  [U]::MoveTo([int](($ro.L + $ro.R) / 2), [int](($ro.T + $ro.B) / 2) + 20)
  Start-Sleep -Milliseconds 100
  [U]::MoveTo([int](($ro.L + $ro.R) / 2) + 5, [int](($ro.T + $ro.B) / 2) + 25)
  Start-Sleep -Milliseconds 500
  Check 'plan8 hover focuses the notepad under the cursor' ((Fg) -eq $other) "fg=$(Fg) expected=$other"
} catch {
  Check 'plan8 section ran without exception' $false "$_"
} finally {
  if ($null -ne $script:heldUp) { [U]::Mouse($script:heldUp); $script:heldUp = $null }
  if ($script:heldAlt) { [U]::Key($ALT, $true); $script:heldAlt = $false }
  $mp | Stop-Process -Force -ErrorAction SilentlyContinue
  Get-Process i3win, i3win-status -ErrorAction SilentlyContinue | Stop-Process -Force
  Remove-Item "$scratch\i3win-live-mouse.conf" -ErrorAction SilentlyContinue
}

# ---- cleanup
$np | Stop-Process -Force -ErrorAction SilentlyContinue
Get-Process i3win, i3win-status -ErrorAction SilentlyContinue | Stop-Process -Force
(New-Object -ComObject Shell.Application).UndoMinimizeALL()
Write-Output '----'
$fail = @($results | Where-Object { $_ -like 'FAIL*' }).Count
Write-Output "$($results.Count - $fail) passed, $fail failed"
Write-Output '--log'
Get-Content "$env:LOCALAPPDATA\i3win\i3win.log" -Tail 20
