use crate::{
    layout,
    market::{Calendar, Session},
    palette::{self, Palette},
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
        UI::{Accessibility::*, HiDpi::*, WindowsAndMessaging::*},
    },
};

const PAGE_INTERVAL: Duration = Duration::from_secs(5);
const QUOTE_INTERVAL: Duration = Duration::from_secs(2);
// Reserved background color; text is drawn without antialiasing against this key.
const TRANSPARENT_KEY: u32 = 0x00010101;
const EXIT_COMMAND: usize = 1;
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
    palette: Palette,
    color_checked: Option<Instant>,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
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
                let started = Instant::now();
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
                    wake.recv_timeout(QUOTE_INTERVAL.saturating_sub(started.elapsed())),
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
            palette: palette::LIGHT_TEXT,
            color_checked: None,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        CONTROLLER.store(hwnd as usize, Ordering::Relaxed);
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

unsafe fn sample_taskbar(
    bar: HWND,
    child: HWND,
    rect: &RECT,
    placement: layout::Placement,
    dpi: i32,
) -> Vec<u32> {
    // Screen pixels include the actual acrylic/wallpaper tint, unlike a guessed
    // Windows theme color. Only sample the verified blank margins beside us.
    let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let mut old: RECT = std::mem::zeroed();
    let exclude_child =
        !child.is_null() && IsWindowVisible(child) != 0 && GetWindowRect(child, &mut old) != 0;
    let dc = GetDC(null_mut());
    let mut samples = Vec::new();
    if !dc.is_null() {
        let offset = (4 * dpi / 96).max(1);
        for x in [
            rect.left + placement.x - offset,
            rect.left + placement.x + placement.width + offset,
        ] {
            for fraction in [1, 2, 3] {
                let y = rect.top + (rect.bottom - rect.top) * fraction / 4;
                if x < rect.left
                    || x >= rect.right
                    || y < rect.top
                    || y >= rect.bottom
                    || (exclude_child
                        && x >= old.left
                        && x < old.right
                        && y >= old.top
                        && y < old.bottom)
                {
                    continue;
                }
                // Ignore menus, other apps or lock-screen surfaces covering the taskbar.
                let hit = WindowFromPoint(POINT { x, y });
                if hit.is_null() || GetAncestor(hit, GA_ROOT) != bar {
                    continue;
                }
                let color = GetPixel(dc, x, y);
                if color != CLR_INVALID {
                    samples.push(color);
                }
            }
        }
        ReleaseDC(null_mut(), dc);
    }
    if !previous.is_null() {
        SetThreadDpiAwarenessContext(previous);
    }
    samples
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
    let dpi = snapshot.dpi.max(96) as i32;
    let dc = CreateCompatibleDC(null_mut());
    if dc.is_null() {
        hide(ptr, "无法测量行情文字宽度");
        return;
    }
    let widths: Option<Vec<i32>> = layout::page((*ptr).page, count, count)
        .into_iter()
        .map(|i| {
            let (text, _) = quote_line(&*ptr, session, i);
            text_width(dc, &text, 13 * dpi / 96).map(|width| width + 2 * (2 * dpi / 96).max(1))
        })
        .collect();
    DeleteDC(dc);
    let Some(widths) = widths else {
        hide(ptr, "无法测量行情文字宽度");
        return;
    };
    let Some(placement) = layout::place_widths(
        rect.right - rect.left,
        &snapshot.occupied,
        &widths,
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
    if (*ptr)
        .color_checked
        .is_none_or(|checked| checked.elapsed() >= Duration::from_secs(1))
    {
        (*ptr).color_checked = Some(Instant::now());
        let samples = sample_taskbar(bar, (*ptr).child, &rect, placement, dpi);
        let palette = palette::choose(&samples, (*ptr).palette);
        if palette != (*ptr).palette {
            (*ptr).palette = palette;
            if !(*ptr).child.is_null() {
                InvalidateRect((*ptr).child, null(), 0);
            }
        }
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
        // Both classic and XAML hosts need a redirected child surface for transparency.
        (*ptr).child = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_LAYERED,
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
            && SetLayeredWindowAttributes((*ptr).child, TRANSPARENT_KEY, 255, LWA_COLORKEY) == 0
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
        let visible_changed = (*ptr).visible != placement.visible;
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
            InvalidateRect((*ptr).child, null(), 0);
        }
        (*ptr).status = if placement.visible == count {
            format!("任务栏内显示全部{count}项")
        } else {
            format!(
                "任务栏内显示{}项 / 共{count}项 · 5秒轮播 · 滚轮切换",
                placement.visible
            )
        };
        if visible_changed {
            InvalidateRect((*ptr).child, null(), 0);
        }
    } else {
        (*ptr).status = format!("嵌入失败（错误 {}）", GetLastError());
    }
    if !previous.is_null() {
        SetThreadDpiAwarenessContext(previous);
    }
}

unsafe fn create_exit_menu() -> HMENU {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return null_mut();
    }
    // A native menu supplies hover/keyboard highlighting and accessibility.
    if AppendMenuW(menu, MF_STRING, EXIT_COMMAND, w!("退出")) == 0 {
        DestroyMenu(menu);
        return null_mut();
    }
    menu
}

unsafe fn menu(hwnd: HWND, position: LPARAM) {
    let menu = create_exit_menu();
    if menu.is_null() {
        return;
    }
    let mut point = POINT {
        x: (position as u32 & 0xffff) as i16 as i32,
        y: ((position as u32 >> 16) & 0xffff) as i16 as i32,
    };
    if position == -1 && GetCursorPos(&mut point) == 0 {
        DestroyMenu(menu);
        return;
    }
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
    // Menu tracking runs a nested message loop. Do not access State afterwards.
    if IsWindow(hwnd) != 0 {
        PostMessageW(hwnd, WM_NULL, 0, 0);
        if selected == EXIT_COMMAND as i32 {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_text_grows_with_content_and_dpi() {
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            assert!(!dc.is_null());
            let short = text_width(dc, "上证 1234.56 +1.23%", 13);
            let long = text_width(dc, "上证 12345.67 +12.34% 旧", 13);
            let scaled = text_width(dc, "上证 1234.56 +1.23%", 26);
            DeleteDC(dc);
            assert!(short.unwrap() > 0);
            assert!(long.unwrap() > short.unwrap());
            assert!(scaled.unwrap() > short.unwrap());
        }
    }

    #[test]
    fn context_menu_contains_only_enabled_exit() {
        unsafe {
            let menu = create_exit_menu();
            assert!(!menu.is_null());
            let count = GetMenuItemCount(menu);
            let id = GetMenuItemID(menu, 0);
            let state = GetMenuState(menu, 0, MF_BYPOSITION);
            let mut label = [0u16; 16];
            let len = GetMenuStringW(
                menu,
                0,
                label.as_mut_ptr(),
                label.len() as i32,
                MF_BYPOSITION,
            );
            DestroyMenu(menu);
            assert_eq!(count, 1);
            assert_eq!(id, EXIT_COMMAND as u32);
            assert_eq!(state & (MF_DISABLED | MF_GRAYED | MF_SEPARATOR), 0);
            assert_eq!(String::from_utf16_lossy(&label[..len as usize]), "退出");
        }
    }
}
unsafe fn advance(ptr: *mut State, direction: i32) {
    if let Some(session) = (*ptr).session {
        let count = quote::indices(session.market).len();
        (*ptr).page = ((*ptr).page as i32 + direction).rem_euclid(count as i32) as usize;
        (*ptr).next_page = Instant::now() + PAGE_INTERVAL;
        attach(ptr);
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
        (*ptr).snapshot = None;
        (*ptr).invalidated = Instant::now();
        return 0;
    }
    match msg {
        WM_TIMER => {
            let mut content_changed = false;
            let session = (*ptr).calendar.active(Utc::now());
            if session != (*ptr).session {
                content_changed = true;
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
                        content_changed |= (*ptr).quotes != q || (*ptr).error.is_some();
                        (*ptr).quotes = q;
                        (*ptr).error = None;
                    }
                    Err(e) => {
                        content_changed |= (*ptr).error.as_ref() != Some(&e);
                        (*ptr).error = Some(e);
                    }
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
                    content_changed |= (*ptr).page != 0;
                    (*ptr).page = 0;
                    (*ptr).next_page = Instant::now() + PAGE_INTERVAL;
                }
            }
            attach(ptr);
            if content_changed && !(*ptr).child.is_null() {
                InvalidateRect((*ptr).child, null(), 0);
            }
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
            0
        }
        LAYOUT_DIRTY | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            (*ptr).color_checked = None;
            (*ptr).invalidated = Instant::now();
            hide(ptr, "任务栏布局变化，重新检测");
            0
        }
        WM_CONTEXTMENU => {
            menu(hwnd, lp);
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
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
unsafe fn text_width(dc: HDC, text: &str, size: i32) -> Option<i32> {
    let font = CreateFontW(
        -size,
        0,
        0,
        0,
        600,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        NONANTIALIASED_QUALITY as u32,
        0,
        w!("Microsoft YaHei UI"),
    );
    if font.is_null() {
        return None;
    }
    let previous = SelectObject(dc, font);
    if previous.is_null() || previous as isize == -1 {
        DeleteObject(font);
        return None;
    }
    let text = wide(text);
    let mut rect: RECT = std::mem::zeroed();
    let measured = DrawTextW(
        dc,
        text.as_ptr(),
        text.len() as i32 - 1,
        &mut rect,
        DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(dc, previous);
    DeleteObject(font);
    (measured > 0 && rect.right > 0).then_some(rect.right)
}

unsafe fn draw(dc: HDC, text: &str, size: i32, rect: &mut RECT, color: u32) {
    let ticker = GetBkColor(dc) == TRANSPARENT_KEY;
    let font = CreateFontW(
        -size,
        0,
        0,
        0,
        if ticker { 600 } else { 400 },
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        // Color-key transparency cannot represent partially covered glyph pixels.
        // Menu DCs keep ClearType; ticker text avoids colored fringes from the key.
        if ticker {
            NONANTIALIASED_QUALITY as u32
        } else {
            CLEARTYPE_QUALITY as u32
        },
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
    let mut rect: RECT = std::mem::zeroed();
    if !dc.is_null() && GetClientRect(hwnd, &mut rect) != 0 && rect.right > 0 && rect.bottom > 0 {
        let buffer = CreateCompatibleDC(dc);
        let bitmap = CreateCompatibleBitmap(dc, rect.right, rect.bottom);
        if !buffer.is_null() && !bitmap.is_null() {
            let previous = SelectObject(buffer, bitmap);
            if !previous.is_null() && previous as isize != -1 {
                render(hwnd, buffer, state);
                // Publish the complete frame, never the intermediate cleared background.
                if BitBlt(dc, 0, 0, rect.right, rect.bottom, buffer, 0, 0, SRCCOPY) == 0 {
                    InvalidateRect(hwnd, null(), 0);
                }
                SelectObject(buffer, previous);
            } else {
                InvalidateRect(hwnd, null(), 0);
            }
        } else {
            InvalidateRect(hwnd, null(), 0);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        if !buffer.is_null() {
            DeleteDC(buffer);
        }
    }
    EndPaint(hwnd, &ps);
}
fn quote_line(state: &State, session: Session, i: usize) -> (String, u32) {
    let index = &quote::indices(session.market)[i];
    let q = state.quotes.get(i).and_then(|q| q.as_ref().ok());
    match q {
        Some(q) => {
            let color = if state.error.is_some() {
                state.palette.muted
            } else if q.percent > 0.0 {
                state.palette.up
            } else if q.percent < 0.0 {
                state.palette.down
            } else {
                state.palette.neutral
            };
            (
                format!(
                    "{} {:.2} {:+.2}%{}",
                    index.name,
                    q.price,
                    q.percent,
                    if state.error.is_some() { " 旧" } else { "" }
                ),
                color,
            )
        }
        None => (
            format!(
                "{} {}",
                index.name,
                state
                    .error
                    .clone()
                    .or_else(|| state.quotes.get(i).and_then(|q| q.as_ref().err().cloned()))
                    .unwrap_or("等待当天行情".into())
            ),
            state.palette.muted,
        ),
    }
}

unsafe fn render(hwnd: HWND, dc: HDC, state: &State) {
    let mut rect: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let saved = SaveDC(dc);
    SetDCBrushColor(dc, TRANSPARENT_KEY);
    FillRect(dc, &rect, GetStockObject(DC_BRUSH) as HBRUSH);
    SetBkColor(dc, TRANSPARENT_KEY);
    SetBkMode(dc, TRANSPARENT as i32);
    if let Some(session) = state.session {
        let list = quote::indices(session.market);
        let visible = state.visible.max(1).min(list.len());
        let mut left = 0;
        let dpi = GetDpiForWindow(hwnd).max(96) as i32;
        // Match the measured widths used by placement; leave only a small gap.
        let font_size = 13 * dpi / 96;
        for i in layout::page(state.page, visible, list.len()) {
            let (text, color) = quote_line(state, session, i);
            let padding = (2 * dpi / 96).max(1);
            let Some(width) = text_width(dc, &text, font_size) else {
                break;
            };
            let right = left + width + 2 * padding;

            draw(
                dc,
                &text,
                font_size,
                &mut RECT {
                    left: left + padding,
                    top: 0,
                    right: right - padding,
                    bottom: rect.bottom,
                },
                color,
            );
            left = right;
        }
    }
    if saved != 0 {
        RestoreDC(dc, saved);
    }
}
