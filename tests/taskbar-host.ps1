# Run only in a service/test desktop without a real taskbar. Never changes Explorer.
param([switch]$HostWindow, [switch]$CompositionHost)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class TestBar {
  public delegate IntPtr Callback(IntPtr h, uint m, IntPtr w, IntPtr l);
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct Class { public uint style; public Callback proc; public int a,b; public IntPtr instance,icon,cursor,brush; public string menu,name; }
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int left,top,right,bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct Point { public int x,y; }
  [StructLayout(LayoutKind.Sequential)] public struct Msg { public IntPtr hwnd; public uint msg; public UIntPtr wp; public IntPtr lp; public uint time; public Point pt; public uint priv; }
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern ushort RegisterClassW(ref Class c);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c,string t);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr p,IntPtr a,string c,string t);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr CreateWindowExW(uint e,string c,string t,uint s,int x,int y,int w,int h,IntPtr p,IntPtr m,IntPtr i,IntPtr l);
  [DllImport("kernel32.dll",CharSet=CharSet.Unicode)] static extern IntPtr GetModuleHandleW(string s);
  [DllImport("user32.dll")] static extern IntPtr DefWindowProcW(IntPtr h,uint m,IntPtr w,IntPtr l);
  [DllImport("user32.dll")] static extern int GetMessageW(out Msg m,IntPtr h,uint a,uint b);
  [DllImport("user32.dll")] static extern bool TranslateMessage(ref Msg m);
  [DllImport("user32.dll")] static extern IntPtr DispatchMessageW(ref Msg m);
  [DllImport("user32.dll")] static extern void PostQuitMessage(int c);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h,uint m,IntPtr w,IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h,uint command);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int w,int height,uint flags);
  [DllImport("user32.dll")] public static extern bool GetLayeredWindowAttributes(IntPtr h,out uint key,out byte alpha,out uint flags);
  [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtrW(IntPtr h,int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out Rect r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int command);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h,int x,int y,int w,int t,bool repaint);
  [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h,StringBuilder b,int n);
  [DllImport("user32.dll")] static extern bool SetProcessDpiAwarenessContext(IntPtr v);
  public static IntPtr Any(string c) { return FindWindowW(c,null); }
  public static IntPtr Child(IntPtr p,string c) { return FindWindowExW(p,IntPtr.Zero,c,null); }
  public static string Text(IntPtr h) { var b=new StringBuilder(1024); GetWindowTextW(h,b,b.Capacity); return b.ToString(); }
  public static string Bounds(IntPtr h) { Rect r; GetWindowRect(h,out r); return r.left+","+r.top+","+r.right+","+r.bottom; }
  static Callback callback = Proc;
  static IntPtr Proc(IntPtr h,uint m,IntPtr w,IntPtr l) { if(m==2) PostQuitMessage(0); return DefWindowProcW(h,m,w,l); }
  public static void Run(bool composition) {
    if(FindWindowW("Shell_TrayWnd",null)!=IntPtr.Zero) throw new Exception("A real/test taskbar already exists; refusing to run");
    SetProcessDpiAwarenessContext(new IntPtr(-4));
    var c=new Class { proc=callback, instance=GetModuleHandleW(null), name="Shell_TrayWnd" };
    if(RegisterClassW(ref c)==0) throw new Exception("RegisterClass failed");
    var h=CreateWindowExW(0,c.name,"IndexTaskbarMockHost",0x90000000,0,0,1280,48,IntPtr.Zero,IntPtr.Zero,c.instance,IntPtr.Zero);
    CreateWindowExW(0,"Button","Existing app icons",0x50000000,0,0,400,48,h,IntPtr.Zero,c.instance,IntPtr.Zero);
    CreateWindowExW(0,"Button","Notification icons",0x50000000,1000,0,280,48,h,IntPtr.Zero,c.instance,IntPtr.Zero);
    if(composition) {
      var bridge=new Class { proc=callback, instance=c.instance, name="Windows.UI.Composition.DesktopWindowContentBridge" };
      if(RegisterClassW(ref bridge)==0) throw new Exception("Register bridge failed");
      CreateWindowExW(0,bridge.name,"",0x50000000,0,0,1,1,h,IntPtr.Zero,c.instance,IntPtr.Zero);
    }
    ShowWindow(h,5);
    Msg m; while(GetMessageW(out m,IntPtr.Zero,0,0)>0) { TranslateMessage(ref m); DispatchMessageW(ref m); }
  }
}
'@
if ($HostWindow) { [TestBar]::Run($CompositionHost.IsPresent); exit }
if ([TestBar]::Any('Shell_TrayWnd') -ne [IntPtr]::Zero) { throw 'Real taskbar exists: this test must run in an isolated desktop' }
if ([TestBar]::FindWindowW('IndexTaskbarController', '指数行情控制器') -ne [IntPtr]::Zero) { throw 'Close the existing index application before testing' }
$hostProcess=$null
$appProcess=$null
function Wait-Until([scriptblock]$Condition, [string]$Message) {
    $deadline=(Get-Date).AddSeconds(12)
    do { if (& $Condition) { return }; Start-Sleep -Milliseconds 150 } while ((Get-Date) -lt $deadline)
    $diagnostics=Join-Path $PSScriptRoot '..\.tools\taskbar-host-diagnostics.txt'
    Start-Process -FilePath (Join-Path $PSScriptRoot '..\dist\sse-taskbar.exe') -ArgumentList '--diagnose',('"' + $diagnostics + '"') -WindowStyle Hidden -Wait
    if (Test-Path $diagnostics) { Get-Content $diagnostics | Write-Output }
    throw $Message
}
function Start-TestHost {
    $arguments=@('-NoProfile','-File',('"' + $PSCommandPath + '"'),'-HostWindow')
    if ($CompositionHost) { $arguments += '-CompositionHost' }
    Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList $arguments -WindowStyle Hidden -PassThru
}
try {
    $hostProcess=Start-TestHost
    Wait-Until { [TestBar]::FindWindowW('Shell_TrayWnd','IndexTaskbarMockHost') -ne [IntPtr]::Zero } 'Mock host did not start'
    $bar=[TestBar]::FindWindowW('Shell_TrayWnd','IndexTaskbarMockHost')
    if (-not [TestBar]::IsWindowVisible($bar)) {
        'SKIP: this desktop does not expose visible windows (for example Windows service Session 0). No GUI integration assertions ran.'
        return
    }
    $left=[TestBar]::FindWindowExW($bar,[IntPtr]::Zero,'Button','Existing app icons')
    $right=[TestBar]::FindWindowExW($bar,[IntPtr]::Zero,'Button','Notification icons')
    $leftBefore=[TestBar]::Bounds($left)
    $rightBefore=[TestBar]::Bounds($right)
    $appProcess=Start-Process -FilePath (Join-Path $PSScriptRoot '..\dist\sse-taskbar.exe') -ArgumentList '--no-autostart' -WindowStyle Hidden -PassThru
    Wait-Until { $script:child=[TestBar]::Child($bar,'IndexTaskbarChild'); $script:child -ne [IntPtr]::Zero -and [TestBar]::IsWindowVisible($script:child) } 'No embedded child appeared (test requires an active market session)'
    if ([TestBar]::GetParent($child) -ne $bar) { throw 'Child parent mismatch' }
    $style=[TestBar]::GetWindowLongPtrW($child,-16).ToInt64()
    $extended=[TestBar]::GetWindowLongPtrW($child,-20).ToInt64()
    if (($style -band 0x40000000) -eq 0 -or ($style -band 0x80000000L) -ne 0 -or ($extended -band 8) -ne 0) { throw 'Not a normal embedded child' }
    if (($extended -band 0x80000) -eq 0) { throw 'Transparent child requires a layered surface on both hosts' }
    [uint32]$key=0; [byte]$alpha=0; [uint32]$flags=0
    if (-not [TestBar]::GetLayeredWindowAttributes($child,[ref]$key,[ref]$alpha,[ref]$flags) -or $key -ne 0x010101 -or $flags -ne 1) { throw 'Child background transparency is not configured' }
    # Simulate Explorer raising a sibling without moving the ticker.
    [void][TestBar]::SetWindowPos($left,[IntPtr]::Zero,0,0,0,0,0x13)
    Wait-Until { [TestBar]::IsWindowVisible($child) -and [TestBar]::GetWindow($child,3) -eq [IntPtr]::Zero } 'Child did not recover its sibling Z order'
    if ([TestBar]::Bounds($left) -ne $leftBefore -or [TestBar]::Bounds($right) -ne $rightBefore) { throw 'System control geometry was modified' }
    Wait-Until { ([TestBar]::Text($child) -split '\|').Count -ge 2 } 'Expected at least two measured-width indices'
    # Find a two-item gap without assuming fixed text widths or a particular market.
    $foundPair=$false
    for ($edge=420; $edge -le 900; $edge+=20) {
        [void][TestBar]::MoveWindow($left,0,0,$edge,48,$true)
        Start-Sleep -Milliseconds 800
        if ([TestBar]::IsWindowVisible($child) -and ([TestBar]::Text($child) -split '\|').Count -eq 2) {
            $foundPair=$true
            break
        }
    }
    if (-not $foundPair) { throw 'Could not find a two-item gap with measured widths' }
    $first=([TestBar]::Text($child) -split ' ')[0]
    Wait-Until { ([TestBar]::Text($child) -split ' ')[0] -ne $first } 'Carousel did not advance'
    $first=([TestBar]::Text($child) -split ' ')[0]
    [void][TestBar]::PostMessageW($child,522,[IntPtr](120 -shl 16),[IntPtr]::Zero)
    Wait-Until { ([TestBar]::Text($child) -split ' ')[0] -ne $first } 'Mouse wheel did not change page'
    [void][TestBar]::MoveWindow($left,0,0,980,48,$true)
    Wait-Until { -not [TestBar]::IsWindowVisible($child) } 'A gap too small for two indices should hide the child'
    [void][TestBar]::MoveWindow($left,0,0,400,48,$true)
    Wait-Until { [TestBar]::IsWindowVisible($child) } 'Child did not return after gap became available'
    if ([TestBar]::Bounds($right) -ne $rightBefore) { throw 'Notification area moved' }
    [void][TestBar]::PostMessageW($bar,16,[IntPtr]::Zero,[IntPtr]::Zero)
    [void]$hostProcess.WaitForExit(5000)
    $hostProcess=Start-TestHost
    Wait-Until { $script:newBar=[TestBar]::FindWindowW('Shell_TrayWnd','IndexTaskbarMockHost'); $script:newBar -ne [IntPtr]::Zero } 'Replacement host did not start'
    Wait-Until { $c=[TestBar]::Child($newBar,'IndexTaskbarChild'); $c -ne [IntPtr]::Zero -and [TestBar]::IsWindowVisible($c) } 'Child did not reconnect after host restart'
    'PASS: child hosting, no popup/topmost style, unchanged controls, automatic carousel, wheel, crowded hiding, recovery, host restart'
} finally {
    if ($appProcess -and -not $appProcess.HasExited) { Stop-Process -Id $appProcess.Id }
    if ($hostProcess -and -not $hostProcess.HasExited) { Stop-Process -Id $hostProcess.Id }
}
