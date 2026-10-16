//! Current-user logon registration. Never touches other applications' values.
use std::{
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::{
    core::w,
    Win32::{Foundation::*, System::Registry::*},
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: windows_sys::core::PCWSTR = w!("SseTaskbar");

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn command(path: &Path) -> Result<Vec<u16>, String> {
    let path: Vec<u16> = path.as_os_str().encode_wide().collect();
    if path.is_empty() || path.iter().any(|&c| c == 0 || c == b'"' as u16) {
        return Err("程序路径无效".into());
    }
    // Run values contain a command line of at most 260 characters, including quotes.
    if path.len() + 2 > 260 {
        return Err("程序路径过长，无法加入开机启动，请移到较短的路径后重新运行".into());
    }
    Ok(std::iter::once(b'"' as u16)
        .chain(path)
        .chain([b'"' as u16, 0])
        .collect())
}

fn open_key(path: &str, options: u32) -> Result<Key, String> {
    let mut key = null_mut();
    let result = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(path).as_ptr(),
            0,
            null(),
            options,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if result != ERROR_SUCCESS {
        return Err(format!("无法打开开机启动设置（错误 {result}）"));
    }
    Ok(Key(key))
}

// Returns whether a missing, stale or malformed entry was repaired.
fn sync_value(key: &Key, command: &[u16]) -> Result<bool, String> {
    let mut current = [0u16; 261];
    let mut bytes = std::mem::size_of_val(&current) as u32;
    let mut kind = 0;
    let result = unsafe {
        RegQueryValueExW(
            key.0,
            VALUE_NAME,
            null(),
            &mut kind,
            current.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if result == ERROR_SUCCESS
        && kind == REG_SZ
        && bytes as usize == command.len() * 2
        && current[..command.len()] == *command
    {
        return Ok(false);
    }
    if !matches!(
        result,
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND | ERROR_MORE_DATA
    ) {
        return Err(format!("无法读取开机启动设置（错误 {result}）"));
    }
    let result = unsafe {
        RegSetValueExW(
            key.0,
            VALUE_NAME,
            0,
            REG_SZ,
            command.as_ptr().cast(),
            std::mem::size_of_val(command) as u32,
        )
    };
    if result != ERROR_SUCCESS {
        return Err(format!("无法保存开机启动设置（错误 {result}）"));
    }
    Ok(true)
}

pub fn ensure_current_exe() -> Result<(), String> {
    let path = std::env::current_exe().map_err(|e| format!("无法读取程序位置：{e}"))?;
    let command = command(&path)?;
    let key = open_key(RUN_KEY, REG_OPTION_NON_VOLATILE)?;
    sync_value(&key, &command)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_spaces_unicode_and_checks_length() {
        let value = command(Path::new(r"D:\行情 工具\sse-taskbar.exe")).unwrap();
        assert_eq!(
            String::from_utf16_lossy(&value[..value.len() - 1]),
            "\"D:\\行情 工具\\sse-taskbar.exe\""
        );
        assert!(command(Path::new(&"a".repeat(258))).is_ok());
        assert!(command(Path::new(&"a".repeat(259))).is_err());
        assert!(command(Path::new("bad\"path")).is_err());
    }

    #[test]
    fn registers_once_and_updates_moved_exe_without_touching_other_values() {
        // A temporary volatile key, never the real Windows Run key.
        let path = format!(
            r"Software\SseTaskbarTest_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    RegDeleteKeyW(HKEY_CURRENT_USER, wide(&self.0).as_ptr());
                }
            }
        }
        let _cleanup = Cleanup(path.clone());
        let key = open_key(&path, REG_OPTION_VOLATILE).unwrap();
        unsafe {
            assert_eq!(
                RegSetValueExW(key.0, w!("OtherApp"), 0, REG_BINARY, [42u8].as_ptr(), 1),
                ERROR_SUCCESS
            );
        }
        let old = command(Path::new(r"D:\old\sse-taskbar.exe")).unwrap();
        let new = command(Path::new(r"D:\新目录\sse-taskbar.exe")).unwrap();
        assert!(sync_value(&key, &old).unwrap());
        assert!(!sync_value(&key, &old).unwrap());
        assert!(sync_value(&key, &new).unwrap());
        assert!(!sync_value(&key, &new).unwrap());
        unsafe {
            let mut data = 0u8;
            let mut bytes = 1;
            assert_eq!(
                RegQueryValueExW(
                    key.0,
                    w!("OtherApp"),
                    null(),
                    null_mut(),
                    &mut data,
                    &mut bytes
                ),
                ERROR_SUCCESS
            );
            assert_eq!(data, 42);
            assert_eq!(
                RegSetValueExW(key.0, VALUE_NAME, 0, REG_BINARY, [0u8].as_ptr(), 1),
                ERROR_SUCCESS
            );
        }
        assert!(sync_value(&key, &new).unwrap());
    }
}
