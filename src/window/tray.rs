use std::{
    ptr::null,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub(super) const CALLBACK: u32 = WM_APP + 3;
const ICON_ID: u32 = 1;
// shellapi.h defines this combination, but windows-sys does not export the macro.
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
const RETRY_INTERVAL: Duration = Duration::from_secs(5);

pub(super) struct Tray {
    added: bool,
    version4: bool,
    next_update: Instant,
    tooltip: String,
}

impl Tray {
    pub(super) fn new() -> Self {
        Self {
            added: false,
            version4: false,
            next_update: Instant::now(),
            tooltip: String::new(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.added = false;
        self.version4 = false;
        self.next_update = Instant::now();
    }

    pub(super) unsafe fn update(&mut self, hwnd: HWND, status: &str) {
        if Instant::now() < self.next_update {
            return;
        }
        self.next_update = Instant::now() + RETRY_INTERVAL;
        let tooltip = format!("指数行情\n{status}");
        if self.added && self.tooltip == tooltip {
            return;
        }
        let mut data = data(hwnd);
        data.uFlags = NIF_TIP | NIF_SHOWTIP;
        // Reserve the final UTF-16 unit for the terminator, without splitting a surrogate pair.
        let mut offset = 0;
        for character in tooltip.chars() {
            if offset + character.len_utf16() >= data.szTip.len() {
                break;
            }
            offset += character.encode_utf16(&mut data.szTip[offset..]).len();
        }
        if !self.added {
            data.uFlags |= NIF_MESSAGE | NIF_ICON;
            data.uCallbackMessage = CALLBACK;
            // LoadIcon returns a shared resource; it must not be destroyed.
            data.hIcon = LoadIconW(GetModuleHandleW(null()), 101usize as *const u16);
            if data.hIcon.is_null() {
                return;
            }
        }
        if Shell_NotifyIconW(if self.added { NIM_MODIFY } else { NIM_ADD }, &data) == 0 {
            // A failed modification may leave the old icon registered. Clear it before retrying.
            Shell_NotifyIconW(NIM_DELETE, &data);
            self.added = false;
            self.version4 = false;
            return;
        }
        if !self.added {
            self.added = true;
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            self.version4 = Shell_NotifyIconW(NIM_SETVERSION, &data) != 0;
        }
        self.tooltip = tooltip;
    }

    pub(super) unsafe fn menu_position(
        &self,
        hwnd: HWND,
        wp: WPARAM,
        lp: LPARAM,
    ) -> Option<LPARAM> {
        let position = decode_menu(self.version4, wp, lp)?;
        if position != -1 {
            return Some(position);
        }
        let id = NOTIFYICONIDENTIFIER {
            cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hwnd,
            uID: ICON_ID,
            ..std::mem::zeroed()
        };
        let mut rect: RECT = std::mem::zeroed();
        if Shell_NotifyIconGetRect(&id, &mut rect) >= 0 {
            return Some(
                ((rect.left as u16 as u32) | ((rect.bottom as u16 as u32) << 16)) as LPARAM,
            );
        }
        Some(-1)
    }

    pub(super) unsafe fn remove(&mut self, hwnd: HWND) {
        Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
        self.added = false;
    }
}

unsafe fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        ..std::mem::zeroed()
    }
}

fn decode_menu(version4: bool, wp: WPARAM, lp: LPARAM) -> Option<LPARAM> {
    if version4 {
        let event = lp as u32 & 0xffff;
        if (lp as u32 >> 16) != ICON_ID {
            return None;
        }
        match event {
            WM_CONTEXTMENU => Some(if wp as u32 == u32::MAX {
                -1
            } else {
                wp as LPARAM
            }),
            NIN_KEYSELECT => Some(-1),
            _ => None,
        }
    } else if wp == ICON_ID as usize && lp as u32 == WM_RBUTTONUP {
        Some(-1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_callbacks_handle_versions_keyboard_and_ignore_other_icons() {
        let event = ((ICON_ID << 16) | WM_CONTEXTMENU) as LPARAM;
        let point = ((20u32 << 16) | (-100i16 as u16 as u32)) as WPARAM;
        assert_eq!(decode_menu(true, point, event), Some(point as LPARAM));
        assert_eq!(decode_menu(true, u32::MAX as WPARAM, event), Some(-1));
        assert_eq!(
            decode_menu(true, 0, ((ICON_ID << 16) | NIN_KEYSELECT) as LPARAM),
            Some(-1)
        );
        assert_eq!(
            decode_menu(true, point, ((2 << 16) | WM_CONTEXTMENU) as LPARAM),
            None
        );
        assert_eq!(
            decode_menu(true, point, ((ICON_ID << 16) | WM_RBUTTONUP) as LPARAM),
            None
        );
        assert_eq!(
            decode_menu(false, ICON_ID as WPARAM, WM_RBUTTONUP as LPARAM),
            Some(-1)
        );
        assert_eq!(
            decode_menu(false, ICON_ID as WPARAM, WM_LBUTTONUP as LPARAM),
            None
        );
    }
}
