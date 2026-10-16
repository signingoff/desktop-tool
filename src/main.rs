#![cfg_attr(windows, windows_subsystem = "windows")]

mod layout;
mod market;
mod quote;
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
    window::run();
    #[cfg(not(windows))]
    eprintln!("请在 Windows 上运行本程序。");
}
