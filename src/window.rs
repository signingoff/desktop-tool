mod menu;
mod render;
mod tray;
mod updates;

use render::{paint, render, TRANSPARENT_KEY};
use updates::Update;

use crate::{
    layout,
    market::{Calendar, Session},
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
        Graphics::Gdi::{InvalidateRect, HDC},
        System::LibraryLoader::GetModuleHandleW,
        UI::{Accessibility::*, HiDpi::*, WindowsAndMessaging::*},
    },
};

const PAGE_INTERVAL: Duration = Duration::from_secs(15);
const LAYOUT_DIRTY: u32 = WM_APP + 2;
static CONTROLLER: AtomicUsize = AtomicUsize::new(0);

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
    tray: tray::Tray,
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
        let (rx, refresh) = updates::start(calendar.clone());
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
            tray: tray::Tray::new(),
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
    event: u32,
    hwnd: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    if !affects_layout(event) {
        return;
    }
    let controller = CONTROLLER.load(Ordering::Relaxed) as HWND;
    if controller.is_null() || hwnd.is_null() {
        return;
    }
    let bar = FindWindowW(w!("Shell_TrayWnd"), null());
    if !bar.is_null() && (hwnd == bar || IsChild(bar, hwnd) != 0) {
        PostMessageW(controller, LAYOUT_DIRTY, 0, 0);
    }
}

fn affects_layout(event: u32) -> bool {
    // Focus, selection and state notifications also fall inside the hook's
    // numeric range. They do not invalidate the measured control boundaries.
    matches!(
        event,
        EVENT_OBJECT_CREATE
            | EVENT_OBJECT_DESTROY
            | EVENT_OBJECT_SHOW
            | EVENT_OBJECT_HIDE
            | EVENT_OBJECT_REORDER
            | EVENT_OBJECT_LOCATIONCHANGE
    )
}

fn input_setting(action: WPARAM) -> bool {
    matches!(action as u32, SPI_SETDEFAULTINPUTLANG | SPI_SETLANGTOGGLE)
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
    let dpi = snapshot.dpi.max(96) as i32;
    let widths = render::measure_widths(&*ptr, session, dpi);
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
        hide(ptr, "任务栏空白不足一项，暂停显示");
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
                "任务栏内显示{}项 / 共{count}项 · 15秒轮播 · 滚轮切换",
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

unsafe fn tick(ptr: *mut State) {
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
        content_changed |= updates::apply_current(
            (*ptr).session,
            &mut (*ptr).quotes,
            &mut (*ptr).error,
            (session, update),
        );
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
    update_accessibility(ptr);
}

unsafe fn update_accessibility(ptr: *mut State) {
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
            let len = GetWindowTextW((*ptr).child, current.as_mut_ptr(), current.len() as i32);
            if String::from_utf16_lossy(&current[..len as usize]) != text {
                SetWindowTextW((*ptr).child, wide(&text).as_ptr());
            }
        }
    }
}

unsafe extern "system" fn controller_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if (*ptr).taskbar_created != 0 && msg == (*ptr).taskbar_created {
        (*ptr).snapshot = None;
        (*ptr).invalidated = Instant::now();
        (*ptr).tray.reset();
        return 0;
    }
    match msg {
        WM_TIMER => {
            tick(ptr);
            (*ptr).tray.update(hwnd);
            0
        }
        WM_SETTINGCHANGE if input_setting(wp) => 0,
        LAYOUT_DIRTY | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            (*ptr).invalidated = Instant::now();
            hide(ptr, "任务栏布局变化，重新检测");
            0
        }
        tray::CALLBACK => {
            // Release all State borrows before entering the menu's nested message loop.
            let position = (*ptr).tray.menu_position(hwnd, wp, lp);
            if let Some(position) = position {
                menu::show(hwnd, position);
            }
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, 1);
            (*ptr).tray.remove(hwnd);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_and_selection_do_not_invalidate_layout() {
        for event in [
            EVENT_OBJECT_FOCUS,
            EVENT_OBJECT_SELECTION,
            EVENT_OBJECT_SELECTIONADD,
            EVENT_OBJECT_SELECTIONREMOVE,
            EVENT_OBJECT_SELECTIONWITHIN,
            EVENT_OBJECT_STATECHANGE,
        ] {
            assert!(!affects_layout(event));
        }
        for event in [
            EVENT_OBJECT_CREATE,
            EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_HIDE,
            EVENT_OBJECT_REORDER,
            EVENT_OBJECT_LOCATIONCHANGE,
        ] {
            assert!(affects_layout(event));
        }
    }

    #[test]
    fn input_language_settings_do_not_invalidate_layout() {
        assert!(input_setting(SPI_SETDEFAULTINPUTLANG as WPARAM));
        assert!(input_setting(SPI_SETLANGTOGGLE as WPARAM));
        assert!(!input_setting(SPI_SETWORKAREA as WPARAM));
        assert!(!input_setting(SPI_SETNONCLIENTMETRICS as WPARAM));
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
        // Do not forward the quote's context menu to the Explorer parent.
        WM_CONTEXTMENU => 0,
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
