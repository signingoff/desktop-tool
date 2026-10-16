use crate::layout::Span;
use std::{
    ptr::null,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::Interface,
    Win32::{Foundation::HWND as UiaHwnd, System::Com::*, UI::Accessibility::*},
};
use windows_sys::{
    core::w,
    Win32::{
        Foundation::*,
        System::Threading::GetCurrentProcessId,
        UI::{HiDpi::*, WindowsAndMessaging::*},
    },
};

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub hwnd: usize,
    pub pid: u32,
    pub rect: [i32; 4],
    pub dpi: u32,
    pub occupied: Vec<Span>,
    pub started: Instant,
}
pub type Scan = Result<Snapshot, String>;

pub unsafe fn has_composition_host(bar: HWND) -> bool {
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> i32 {
        let class = class_name(hwnd);
        if matches!(
            class.as_str(),
            "Windows.UI.Composition.DesktopWindowContentBridge"
                | "Microsoft.UI.Content.DesktopChildSiteBridge"
                | "Windows.UI.Input.InputSite.WindowClass"
        ) {
            *(data as *mut bool) = true;
            return 0;
        }
        1
    }
    let mut found = false;
    EnumChildWindows(bar, Some(visit), &mut found as *mut bool as LPARAM);
    found
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

unsafe fn native_report() -> String {
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> i32 {
        let output = &mut *(data as *mut String);
        if output.len() > 32_768 {
            return 0;
        }
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        output.push_str(&format!("class={} hwnd={:?} parent={:?} prev={:?} visible={} style={:#x} exstyle={:#x} rect={:?}\n",
            class_name(hwnd), hwnd, GetParent(hwnd), GetWindow(hwnd, GW_HWNDPREV), IsWindowVisible(hwnd),
            GetWindowLongPtrW(hwnd, GWL_STYLE), GetWindowLongPtrW(hwnd, GWL_EXSTYLE),
            [r.left, r.top, r.right, r.bottom]));
        1
    }
    let bar = FindWindowW(w!("Shell_TrayWnd"), null());
    let mut report = format!(
        "Composition host: {}\n",
        !bar.is_null() && has_composition_host(bar)
    );
    if !bar.is_null() {
        visit(bar, &mut report as *mut String as LPARAM);
        EnumChildWindows(bar, Some(visit), &mut report as *mut String as LPARAM);
    }
    report
}

pub fn diagnose(path: &str) {
    let stop = Arc::new(AtomicBool::new(false));
    let rx = start(stop.clone());
    let report = rx.recv_timeout(Duration::from_secs(5));
    stop.store(true, Ordering::Relaxed);
    let session = crate::market::Calendar::load().map(|c| c.active(chrono::Utc::now()));
    let _ = std::fs::write(
        path,
        format!(
            "Version: {}\nSession: {session:?}\nTaskbar: {report:#?}\nNative windows:\n{}",
            env!("CARGO_PKG_VERSION"),
            unsafe { native_report() }
        ),
    );
}

pub fn start(stop: Arc<AtomicBool>) -> mpsc::Receiver<Scan> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            let _ = tx.send(Err("无法初始化任务栏空间检测".into()));
            return;
        }
        {
            let automation: windows::core::Result<IUIAutomation> =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER);
            if let Ok(ref automation) = automation {
                if let Ok(settings) = automation.cast::<IUIAutomation2>() {
                    let _ = settings.SetConnectionTimeout(500);
                    let _ = settings.SetTransactionTimeout(500);
                }
            }
            while !stop.load(Ordering::Relaxed) {
                let result = match &automation {
                    Ok(a) => scan(a),
                    Err(_) => Err("系统不支持任务栏空间检测".into()),
                };
                match tx.try_send(result) {
                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                    _ => {}
                }
                std::thread::sleep(Duration::from_millis(300));
            }
        }
        CoUninitialize();
    });
    rx
}

unsafe fn scan(automation: &IUIAutomation) -> Scan {
    let started = Instant::now();
    let hwnd = FindWindowW(w!("Shell_TrayWnd"), null());
    if hwnd.is_null() {
        return Err("未找到当前桌面的任务栏".into());
    }
    let prior = SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(hwnd));
    let result = scan_inner(automation, hwnd, started);
    if !prior.is_null() {
        SetThreadDpiAwarenessContext(prior);
    }
    result
}

unsafe fn scan_inner(automation: &IUIAutomation, hwnd: HWND, started: Instant) -> Scan {
    let mut rect: RECT = std::mem::zeroed();
    if IsWindowVisible(hwnd) == 0 || GetWindowRect(hwnd, &mut rect) == 0 {
        return Err("任务栏不可见".into());
    }
    if rect.right - rect.left <= rect.bottom - rect.top {
        return Err("暂不支持竖向任务栏".into());
    }
    let root = automation
        .ElementFromHandle(UiaHwnd(hwnd))
        .map_err(|_| "无法读取任务栏控件")?;
    let condition = automation
        .CreateTrueCondition()
        .map_err(|_| "无法读取任务栏控件")?;
    let mut occupied = Vec::new();
    let mut nodes = 0usize;
    collect(&root, &condition, rect, &mut occupied, &mut nodes, 0)?;
    if occupied.is_empty() {
        return Err("未识别出任务栏按钮，已停止显示".into());
    }
    // Reserve native notification and Start regions even if their provider only exposes text.
    for class in [
        w!("TrayNotifyWnd"),
        w!("Start"),
        w!("TrayShowDesktopButton"),
    ] {
        let child = FindWindowExW(hwnd, std::ptr::null_mut(), class, null());
        let mut bounds = std::mem::zeroed();
        if !child.is_null() && IsWindowVisible(child) != 0 && GetWindowRect(child, &mut bounds) != 0
        {
            occupied.push(Span {
                start: bounds.left - rect.left,
                end: bounds.right - rect.left,
            });
        }
    }
    let mut latest = std::mem::zeroed();
    if GetWindowRect(hwnd, &mut latest) == 0
        || [latest.left, latest.top, latest.right, latest.bottom]
            != [rect.left, rect.top, rect.right, rect.bottom]
    {
        return Err("任务栏布局正在变化".into());
    }
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    Ok(Snapshot {
        hwnd: hwnd as usize,
        pid,
        rect: [rect.left, rect.top, rect.right, rect.bottom],
        dpi: GetDpiForWindow(hwnd).max(96),
        occupied,
        started,
    })
}

unsafe fn collect(
    element: &IUIAutomationElement,
    condition: &IUIAutomationCondition,
    bar: RECT,
    occupied: &mut Vec<Span>,
    nodes: &mut usize,
    depth: usize,
) -> Result<(), String> {
    *nodes += 1;
    if *nodes > 2048 || depth > 24 {
        return Err("任务栏控件结构超出识别范围".into());
    }
    if element.CurrentProcessId().map_err(|_| "任务栏控件已变化")? as u32 == GetCurrentProcessId()
    {
        return Ok(());
    }
    if element
        .CurrentIsOffscreen()
        .map_err(|_| "任务栏控件状态不可用")?
        .as_bool()
    {
        return Ok(());
    }
    let kind = element
        .CurrentControlType()
        .map_err(|_| "任务栏控件类型不可用")?;
    let container = [
        UIA_PaneControlTypeId,
        UIA_WindowControlTypeId,
        UIA_GroupControlTypeId,
        UIA_ToolBarControlTypeId,
        UIA_ListControlTypeId,
        UIA_TabControlTypeId,
    ]
    .contains(&kind);
    if depth == 0 || container {
        let children = element
            .FindAll(TreeScope_Children, condition)
            .map_err(|_| "无法读取任务栏子控件")?;
        let count = children.Length().map_err(|_| "无法读取任务栏子控件")?;
        if count > 0 {
            for i in 0..count {
                collect(
                    &children.GetElement(i).map_err(|_| "任务栏控件已变化")?,
                    condition,
                    bar,
                    occupied,
                    nodes,
                    depth + 1,
                )?;
            }
            return Ok(());
        }
    }
    // Unknown visible leaf controls are occupied too, never assumed to be blank space.
    let r = element
        .CurrentBoundingRectangle()
        .map_err(|_| "无法读取控件边界")?;
    if r.right > r.left && r.bottom > bar.top && r.top < bar.bottom {
        occupied.push(Span {
            start: r.left - bar.left,
            end: r.right - bar.left,
        });
    }
    Ok(())
}
