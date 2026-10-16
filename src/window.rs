use crate::{
    layout,
    market::{Calendar, Market, Session},
    quote::{self, Quotes},
    taskbar::{self, Scan, Snapshot},
};
use chrono::Utc;
use std::{
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use windows_sys::{
    core::w,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{Accessibility::*, Controls::*, HiDpi::*, Shell::*, WindowsAndMessaging::*},
    },
};

const PAGE_INTERVAL: Duration = Duration::from_millis(1500);
const TRAY: u32 = WM_APP + 1;
const LAYOUT_DIRTY: u32 = WM_APP + 2;
static CONTROLLER: AtomicUsize = AtomicUsize::new(0);
type Update = (Session, Result<Quotes, String>);

struct State {
    calendar: Arc<Calendar>,
    session: Option<Session>,
    quotes: Quotes,
    error: Option<String>,
    updates: mpsc::Receiver<Update>,
    refresh: mpsc::SyncSender<()>,
    scans: mpsc::Receiver<Scan>,
    snapshot: Option<Snapshot>,
    stop: Arc<AtomicBool>,
    child: HWND,
    visible: usize,
    page: usize,
    next_page: Instant,
    invalidated: Instant,
    status: String,
    taskbar_created: u32,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe fn tray(hwnd: HWND, operation: u32, status: &str) {
    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = TRAY;
    data.hIcon = LoadImageW(
        GetModuleHandleW(null()),
        101usize as windows_sys::core::PCWSTR,
        IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        LR_SHARED,
    ) as HICON;
    let tip = wide(&format!("指数行情 · {status}"));
    let len = tip.len().min(data.szTip.len() - 1);
    data.szTip[..len].copy_from_slice(&tip[..len]);
    Shell_NotifyIconW(operation, &data);
}

pub fn run() {
    unsafe {
        if !FindWindowW(w!("IndexTaskbarController"), null()).is_null() {
            return;
        }
        let calendar = match Calendar::load() {
            Ok(c) => Arc::new(c),
            Err(e) => {
                MessageBoxW(
                    null_mut(),
                    wide(&e).as_ptr(),
                    w!("指数行情"),
                    MB_OK | MB_ICONERROR,
                );
                return;
            }
        };
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let (tx, rx) = mpsc::channel();
        let (refresh, wake) = mpsc::sync_channel(1);
        let worker_calendar = calendar.clone();
        std::thread::spawn(move || {
            let agent = quote::agent();
            loop {
                if let Some(session) = worker_calendar.active(Utc::now()) {
                    let result = agent
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(|a| quote::fetch(a, session));
                    if tx.send((session, result)).is_err() {
                        break;
                    }
                }
                if matches!(
                    wake.recv_timeout(Duration::from_secs(10)),
                    Err(mpsc::RecvTimeoutError::Disconnected)
                ) {
                    break;
                }
            }
        });
        let stop = Arc::new(AtomicBool::new(false));
        let scans = taskbar::start(stop.clone());
        let instance = GetModuleHandleW(null());
        let classes: [(windows_sys::core::PCWSTR, WNDPROC); 2] = [
            (w!("IndexTaskbarController"), Some(controller_proc)),
            (w!("IndexTaskbarChild"), Some(child_proc)),
        ];
        for (name, callback) in classes {
            let class = WNDCLASSW {
                lpfnWndProc: callback,
                hInstance: instance,
                lpszClassName: name,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                hIcon: LoadIconW(instance, 101usize as windows_sys::core::PCWSTR),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&class) == 0 {
                stop.store(true, Ordering::Relaxed);
                return;
            }
        }
        // Hidden controller only; it is never shown or positioned over the taskbar.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("IndexTaskbarController"),
            w!("指数行情控制器"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            stop.store(true, Ordering::Relaxed);
            return;
        }
        let state = Box::new(State {
            session: calendar.active(Utc::now()),
            calendar,
            quotes: Vec::new(),
            error: None,
            updates: rx,
            refresh,
            scans,
            snapshot: None,
            stop,
            child: null_mut(),
            visible: 0,
            page: 0,
            next_page: Instant::now() + PAGE_INTERVAL,
            invalidated: Instant::now(),
            status: "正在检测任务栏空白区域".into(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        CONTROLLER.store(hwnd as usize, Ordering::Relaxed);
        tray(hwnd, NIM_ADD, "正在检测任务栏空白区域");
        let hook = SetWinEventHook(
            EVENT_OBJECT_CREATE,
            EVENT_OBJECT_LOCATIONCHANGE,
            null_mut(),
            Some(layout_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        SetTimer(hwnd, 1, 100, None);
        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        CONTROLLER.store(0, Ordering::Relaxed);
        if !hook.is_null() {
            UnhookWinEvent(hook);
        }
    }
}

unsafe extern "system" fn layout_event(
    _: HWINEVENTHOOK,
    _: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    let controller = CONTROLLER.load(Ordering::Relaxed) as HWND;
    if controller.is_null() || hwnd.is_null() {
        return;
    }
    let bar = FindWindowW(w!("Shell_TrayWnd"), null());
    if !bar.is_null() && (hwnd == bar || IsChild(bar, hwnd) != 0) {
        PostMessageW(controller, LAYOUT_DIRTY, 0, 0);
    }
}

unsafe fn hide(ptr: *mut State, reason: &str) {
    (*ptr).status = reason.into();
    let child = (*ptr).child;
    if !child.is_null() && IsWindow(child) != 0 {
        ShowWindow(child, SW_HIDE);
    }
}

unsafe fn attach(ptr: *mut State) {
    let Some(session) = (*ptr).session else {
        hide(ptr, "休市或日历未覆盖当前年份");
        return;
    };
    let Some(snapshot) = (*ptr).snapshot.clone() else {
        return;
    };
    // Never reuse a slow/stale scan or one captured before a layout-change event.
    if snapshot.started.elapsed() > Duration::from_millis(1500)
        || snapshot.started < (*ptr).invalidated
    {
        hide(ptr, "等待稳定的任务栏布局");
        return;
    }
    let bar = snapshot.hwnd as HWND;
    let mut pid = 0;
    GetWindowThreadProcessId(bar, &mut pid);
    let mut rect: RECT = std::mem::zeroed();
    if FindWindowW(w!("Shell_TrayWnd"), null()) != bar
        || pid != snapshot.pid
        || IsWindowVisible(bar) == 0
        || GetWindowRect(bar, &mut rect) == 0
        || [rect.left, rect.top, rect.right, rect.bottom] != snapshot.rect
    {
        hide(ptr, "任务栏已变化，等待重新连接");
        return;
    }
    let count = quote::indices(session.market).len();
    let cell = 190 * snapshot.dpi as i32 / 96;
    let Some(placement) = layout::place(
        rect.right - rect.left,
        &snapshot.occupied,
        cell,
        count,
        8 * snapshot.dpi as i32 / 96,
    ) else {
        hide(ptr, "任务栏空白不足两项，暂停显示");
        return;
    };
    let height = (40 * snapshot.dpi as i32 / 96).min(rect.bottom - rect.top);
    if height < 24 {
        hide(ptr, "任务栏高度不足");
        return;
    }
    // Cross-process child creation uses the parent's DPI context. No SetParent fallback.
    let previous = SetThreadDpiAwarenessContext(GetWindowDpiAwarenessContext(bar));
    if !(*ptr).child.is_null() && (IsWindow((*ptr).child) == 0 || GetParent((*ptr).child) != bar) {
        let old = (*ptr).child;
        (*ptr).child = null_mut();
        if IsWindow(old) != 0 {
            DestroyWindow(old);
        }
    }
    if (*ptr).child.is_null() {
        // XAML taskbars use composition surfaces. Give our child its own redirected
        // surface; classic taskbars retain the existing GDI child path.
        let layered = taskbar::has_composition_host(bar);
        (*ptr).child = CreateWindowExW(
            WS_EX_NOACTIVATE | if layered { WS_EX_LAYERED } else { 0 },
            w!("IndexTaskbarChild"),
            w!("指数行情"),
            WS_CHILD | WS_CLIPSIBLINGS,
            placement.x,
            0,
            placement.width,
            height,
            bar,
            null_mut(),
            GetModuleHandleW(null()),
            ptr.cast(),
        );
        if !(*ptr).child.is_null()
            && layered
            && SetLayeredWindowAttributes((*ptr).child, 0, 255, LWA_ALPHA) == 0
        {
            let error = GetLastError();
            DestroyWindow((*ptr).child);
            (*ptr).child = null_mut();
            (*ptr).status = format!("子窗口合成失败（错误 {error}）");
            if !previous.is_null() {
                SetThreadDpiAwarenessContext(previous);
            }
            return;
        }
    }
    if !(*ptr).child.is_null() {
        (*ptr).visible = placement.visible;
        let y = (rect.bottom - rect.top - height) / 2;
        let mut current: RECT = std::mem::zeroed();
        GetWindowRect((*ptr).child, &mut current);
        if IsWindowVisible((*ptr).child) == 0
            || !GetWindow((*ptr).child, GW_HWNDPREV).is_null()
            || current.left != rect.left + placement.x
            || current.top != rect.top + y
            || current.right - current.left != placement.width
            || current.bottom - current.top != height
        {
            let positioned = SetWindowPos(
                (*ptr).child,
                HWND_TOP,
                placement.x,
                y,
                placement.width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            if positioned == 0 {
                let error = GetLastError();
                hide(ptr, &format!("子窗口定位失败（错误 {error}）"));
                if !previous.is_null() {
                    SetThreadDpiAwarenessContext(previous);
                }
                return;
            }
        }
        (*ptr).status = if placement.visible == count {
            format!("任务栏内显示全部{count}项")
        } else {
            format!(
                "任务栏内显示{}项 / 共{count}项 · 1.5秒轮播 · 滚轮切换",
                placement.visible
            )
        };
        InvalidateRect((*ptr).child, null(), 0);
    } else {
        (*ptr).status = format!("嵌入失败（错误 {}）", GetLastError());
    }
    if !previous.is_null() {
        SetThreadDpiAwarenessContext(previous);
    }
}

unsafe fn menu(hwnd: HWND, ptr: *mut State) {
    let menu = CreatePopupMenu();
    AppendMenuW(
        menu,
        MF_STRING | MF_DISABLED,
        0,
        wide(&(*ptr).status).as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    let dpi = (*ptr).snapshot.as_ref().map_or(96, |s| s.dpi);
    for id in [1, 2, 4, 3] {
        // Keep the native menu string for accessibility; only customize its paint.
        AppendMenuW(menu, MF_STRING, id as usize, wide(menu_label(id)).as_ptr());
        let info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_DATA,
            fType: MFT_OWNERDRAW,
            dwItemData: dpi as usize,
            ..std::mem::zeroed()
        };
        SetMenuItemInfoW(menu, id, 0, &info);
    }
    let mut point = std::mem::zeroed();
    GetCursorPos(&mut point);
    SetForegroundWindow(hwnd);
    let selected = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        0,
        hwnd,
        null(),
    );
    DestroyMenu(menu);
    PostMessageW(hwnd, WM_NULL, 0, 0);
    match selected {
        1 => {
            let _ = (*ptr).refresh.try_send(());
        }
        2 => advance(ptr, 1),
        3 => {
            DestroyWindow(hwnd);
        }
        4 => {
            MessageBoxW(hwnd, w!("行情仅嵌入任务栏空白区域，不移动现有图标。\n空间不足时每1.5秒轮播，滚轮可切换。\nA股交易日09:30开始，收盘后保留至美股开盘。\n美股按美东09:30至16:00，提前收市日13:00结束。\n内置日历：A股2026；美股2026—2028。\n行情可能延迟，下方显示源行情时间。"), w!("指数行情"), MB_OK);
        }
        _ => {}
    }
}
fn menu_label(id: u32) -> &'static str {
    match id {
        1 => "立即刷新",
        2 => "下一页行情",
        4 => "显示规则",
        3 => "退出",
        _ => "",
    }
}

unsafe fn draw_menu_item(item: &DRAWITEMSTRUCT) {
    let saved = SaveDC(item.hDC);
    let disabled = item.itemState & (ODS_DISABLED | ODS_GRAYED) != 0;
    let selected = !disabled && item.itemState & ODS_SELECTED != 0;
    // Native selection messages cover mouse hover, keyboard navigation and leave.
    FillRect(
        item.hDC,
        &item.rcItem,
        GetSysColorBrush(if selected {
            COLOR_HIGHLIGHT
        } else {
            COLOR_MENU
        }),
    );
    let color = GetSysColor(if disabled {
        COLOR_GRAYTEXT
    } else if selected {
        COLOR_HIGHLIGHTTEXT
    } else {
        COLOR_MENUTEXT
    });
    let dpi = (item.itemData as i32).max(96);
    let mut rect = item.rcItem;
    rect.left += 12 * dpi / 96;
    rect.right -= 12 * dpi / 96;
    SetBkMode(item.hDC, TRANSPARENT as i32);
    draw(
        item.hDC,
        menu_label(item.itemID),
        14 * dpi / 96,
        &mut rect,
        color,
    );
    if saved != 0 {
        RestoreDC(item.hDC, saved);
    }
}

unsafe fn advance(ptr: *mut State, direction: i32) {
    if let Some(session) = (*ptr).session {
        let count = quote::indices(session.market).len();
        (*ptr).page = ((*ptr).page as i32 + direction).rem_euclid(count as i32) as usize;
        (*ptr).next_page = Instant::now() + PAGE_INTERVAL;
        if !(*ptr).child.is_null() {
            InvalidateRect((*ptr).child, null(), 0);
        }
    }
}

unsafe extern "system" fn controller_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == (*ptr).taskbar_created {
        tray(hwnd, NIM_ADD, "任务栏重建，等待连接");
        (*ptr).snapshot = None;
        (*ptr).invalidated = Instant::now();
        return 0;
    }
    match msg {
        WM_MEASUREITEM if lp != 0 => {
            let item = &mut *(lp as *mut MEASUREITEMSTRUCT);
            if item.CtlType != ODT_MENU {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            let dpi = (item.itemData as u32).max(96);
            item.itemWidth = 140 * dpi / 96;
            item.itemHeight = 32 * dpi / 96;
            1
        }
        WM_DRAWITEM if lp != 0 => {
            let item = &*(lp as *const DRAWITEMSTRUCT);
            if item.CtlType != ODT_MENU {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            draw_menu_item(item);
            1
        }
        WM_TIMER => {
            let old_status = (*ptr).status.clone();
            let session = (*ptr).calendar.active(Utc::now());
            if session != (*ptr).session {
                (*ptr).session = session;
                (*ptr).quotes.clear();
                (*ptr).error = None;
                (*ptr).page = 0;
                let _ = (*ptr).refresh.try_send(());
            }
            while let Ok((session, update)) = (*ptr).updates.try_recv() {
                if Some(session) != (*ptr).session {
                    continue;
                }
                match update {
                    Ok(q) => {
                        (*ptr).quotes = q;
                        (*ptr).error = None;
                    }
                    Err(e) => (*ptr).error = Some(e),
                }
            }
            while let Ok(scan) = (*ptr).scans.try_recv() {
                match scan {
                    Ok(s) => (*ptr).snapshot = Some(s),
                    Err(e) => {
                        (*ptr).snapshot = None;
                        hide(ptr, &e);
                    }
                }
            }
            if Instant::now() >= (*ptr).next_page {
                if (*ptr)
                    .session
                    .is_some_and(|s| (*ptr).visible < quote::indices(s.market).len())
                {
                    advance(ptr, 1);
                } else {
                    (*ptr).page = 0;
                    (*ptr).next_page = Instant::now() + PAGE_INTERVAL;
                }
            }
            attach(ptr);
            // Expose the currently visible values to accessibility clients, too.
            if let Some(session) = (*ptr).session {
                if !(*ptr).child.is_null() {
                    let list = quote::indices(session.market);
                    let text = layout::page((*ptr).page, (*ptr).visible, list.len())
                        .iter()
                        .map(
                            |&i| match (&(*ptr).quotes).get(i).and_then(|q| q.as_ref().ok()) {
                                Some(q) => {
                                    format!("{} {:.2} {:+.2}%", list[i].name, q.price, q.percent)
                                }
                                None => format!("{} 等待行情", list[i].name),
                            },
                        )
                        .collect::<Vec<_>>()
                        .join(" | ");
                    let mut current = [0u16; 512];
                    let len =
                        GetWindowTextW((*ptr).child, current.as_mut_ptr(), current.len() as i32);
                    if String::from_utf16_lossy(&current[..len as usize]) != text {
                        SetWindowTextW((*ptr).child, wide(&text).as_ptr());
                    }
                }
            }
            if old_status != (*ptr).status {
                tray(hwnd, NIM_MODIFY, &(*ptr).status);
            }
            0
        }
        LAYOUT_DIRTY | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            (*ptr).invalidated = Instant::now();
            hide(ptr, "任务栏布局变化，重新检测");
            0
        }
        TRAY => {
            if lp as u32 == WM_RBUTTONUP || lp as u32 == WM_LBUTTONUP {
                menu(hwnd, ptr);
            }
            0
        }
        WM_CONTEXTMENU => {
            menu(hwnd, ptr);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            tray(hwnd, NIM_DELETE, "");
            (*ptr).stop.store(true, Ordering::Relaxed);
            if !(*ptr).child.is_null() && IsWindow((*ptr).child) != 0 {
                DestroyWindow((*ptr).child);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn child_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    match msg {
        WM_PAINT => {
            paint(hwnd, &*ptr);
            0
        }
        WM_PRINTCLIENT => {
            render(hwnd, wp as HDC, &*ptr);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_MOUSEWHEEL => {
            advance(ptr, if ((wp >> 16) as i16) > 0 { -1 } else { 1 });
            0
        }
        WM_CONTEXTMENU => {
            PostMessageW(
                CONTROLLER.load(Ordering::Relaxed) as HWND,
                WM_CONTEXTMENU,
                0,
                lp,
            );
            0
        }
        WM_NCDESTROY => {
            if (*ptr).child == hwnd {
                (*ptr).child = null_mut();
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
unsafe fn draw(dc: HDC, text: &str, size: i32, rect: &mut RECT, color: u32) {
    let font = CreateFontW(
        -size,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        w!("Microsoft YaHei UI"),
    );
    let previous = SelectObject(dc, font);
    SetTextColor(dc, color);
    let text = wide(text);
    DrawTextW(
        dc,
        text.as_ptr(),
        text.len() as i32 - 1,
        rect,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    SelectObject(dc, previous);
    DeleteObject(font);
}
unsafe fn paint(hwnd: HWND, state: &State) {
    let mut ps = std::mem::zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    render(hwnd, dc, state);
    EndPaint(hwnd, &ps);
}
unsafe fn render(hwnd: HWND, dc: HDC, state: &State) {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let brush = CreateSolidBrush(0x00202020);
    FillRect(dc, &rect, brush);
    DeleteObject(brush);
    SetBkMode(dc, TRANSPARENT as i32);
    if let Some(session) = state.session {
        let list = quote::indices(session.market);
        let visible = state.visible.max(1).min(list.len());
        let cell = rect.right / visible as i32;
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        // Compress text on smaller screens while preserving every index.
        let scale = |n: i32| (n * dpi / 96).min(n * cell / 184).max(8);
        for (slot, i) in layout::page(state.page, visible, list.len())
            .into_iter()
            .enumerate()
        {
            let index = &list[i];
            let q = state.quotes.get(i).and_then(|q| q.as_ref().ok());
            let (top, bottom, color) = match q {
                Some(q) => {
                    let color = if state.error.is_some() {
                        0x00909090
                    } else if q.percent > 0.0 {
                        0x006B6BFF
                    } else if q.percent < 0.0 {
                        0x008FD35C
                    } else {
                        0x00EEEEEE
                    };
                    (
                        format!("{}  {:.2}  {:+.2}%", index.name, q.price, q.percent),
                        format!(
                            "{} {}",
                            if state.error.is_some() {
                                "旧"
                            } else if session.market == Market::Us {
                                "美东"
                            } else {
                                "行情"
                            },
                            q.time.format("%m-%d %H:%M:%S")
                        ),
                        color,
                    )
                }
                None => (
                    format!("{}  --  --%", index.name),
                    state
                        .error
                        .clone()
                        .or_else(|| state.quotes.get(i).and_then(|q| q.as_ref().err().cloned()))
                        .unwrap_or("等待当天行情".into()),
                    0x00AAAAAA,
                ),
            };
            let left = slot as i32 * cell + 5;
            let right = (slot as i32 + 1) * cell - 4;
            draw(
                dc,
                &top,
                scale(13),
                &mut RECT {
                    left,
                    top: 0,
                    right,
                    bottom: rect.bottom * 3 / 5,
                },
                color,
            );
            draw(
                dc,
                &bottom,
                scale(10),
                &mut RECT {
                    left,
                    top: rect.bottom * 3 / 5,
                    right,
                    bottom: rect.bottom,
                },
                0x00AAAAAA,
            );
        }
    }
}
