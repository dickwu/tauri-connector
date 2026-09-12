# Actual Windows user32 input for the isolated fixture only.
param([string]$Action, [int]$X, [int]$Y)
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class FixtureInput {
  [StructLayout(LayoutKind.Sequential)] public struct Point { public int X,Y; }
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr window,ref Point point);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr window,out Rect rect);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint dx,uint dy,uint data,UIntPtr extra);
  [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,UIntPtr extra);
}
'@
if ($Action -eq 'window') {
  $fixture = Get-Process -Id $X -ErrorAction Stop
  $window = $fixture.MainWindowHandle
  if ($window -eq [IntPtr]::Zero) { throw 'fixture_window_unavailable' }
  [void][FixtureInput]::SetThreadDpiAwarenessContext([IntPtr](-4))
  [void][FixtureInput]::SetForegroundWindow($window)
  $point = New-Object FixtureInput+Point
  $rect = New-Object FixtureInput+Rect
  if (-not [FixtureInput]::ClientToScreen($window,[ref]$point)) {throw 'fixture_client_origin_unavailable'}
  if (-not [FixtureInput]::GetClientRect($window,[ref]$rect)) {throw 'fixture_client_extent_unavailable'}
  [pscustomobject]@{x=$point.X;y=$point.Y;width=$rect.Right-$rect.Left;height=$rect.Bottom-$rect.Top;scale=([FixtureInput]::GetDpiForWindow($window)/96.0)} | ConvertTo-Json -Compress
} elseif ($Action -eq 'escape') {
  [FixtureInput]::keybd_event(0x1b,0,0,[UIntPtr]::Zero)
  Start-Sleep -Milliseconds 80
  [FixtureInput]::keybd_event(0x1b,0,2,[UIntPtr]::Zero)
} elseif ($Action -ne 'probe') {
  if (-not [FixtureInput]::SetCursorPos($X,$Y)) { throw 'native_input_unavailable' }
  $count = if ($Action -eq 'double') {2} else {1}
  1..$count | ForEach-Object {
    [FixtureInput]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
    Start-Sleep -Milliseconds 30
    [FixtureInput]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
  }
}
