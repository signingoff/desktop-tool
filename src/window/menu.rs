use std::ptr::{null, null_mut};
use windows_sys::{
    core::w,
    Win32::{Foundation::*, UI::WindowsAndMessaging::*},
};

const EXIT_COMMAND: usize = 1;

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

pub(super) unsafe fn show(hwnd: HWND, position: LPARAM) {
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
