#![cfg_attr(windows, windows_subsystem = "windows")]

mod layout;
mod market;
mod quote;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod taskbar;
#[cfg(windows)]
mod window;

fn main() {
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--diagnose") {
        let path = std::env::args()
            .nth(2)
            .unwrap_or_else(|| "taskbar-diagnostics.txt".into());
        taskbar::diagnose(&path);
        return;
    }
    #[cfg(windows)]
    {
        // Register before the single-instance check: running a moved copy updates
        // the path even if an older copy is still showing the ticker.
        if !std::env::args().any(|arg| arg == "--no-autostart") {
            if let Err(error) = startup::ensure_current_exe() {
                let text: Vec<u16> = format!("{error}\n行情仍可运行，但开机启动未能更新。")
                    .encode_utf16()
                    .chain(Some(0))
                    .collect();
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                        std::ptr::null_mut(),
                        text.as_ptr(),
                        windows_sys::core::w!("指数行情"),
                        windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                            | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONWARNING,
                    );
                }
            }
        }
        window::run();
    }
    #[cfg(not(windows))]
    eprintln!("请在 Windows 上运行本程序。");
}
