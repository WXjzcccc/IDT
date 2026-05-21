use std::path::Path;

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND},
        System::StationsAndDesktops::{
            CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_SWITCHDESKTOP, OpenInputDesktop,
            SwitchDesktop,
        },
        System::Threading::{
            GetCurrentProcessId, OpenProcess, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            GetClassNameW, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
            GetWindowThreadProcessId, IsWindowVisible,
        },
    },
    core::PWSTR,
};

use crate::db::FocusInfo;

pub fn current_foreground() -> Option<FocusInfo> {
    if is_workstation_locked() {
        return None;
    }

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() {
            return None;
        }

        let mut process_id = 0_u32;
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
        if process_id == 0 || process_id == GetCurrentProcessId() {
            return None;
        }

        let exe_path = process_path(process_id).unwrap_or_default();
        let process_name = process_name(&exe_path).unwrap_or_else(|| format!("pid-{process_id}"));
        if is_lock_screen_process(&process_name) {
            return None;
        }

        let window_title = window_text(hwnd).unwrap_or_else(|| process_name.clone());
        let window_class = class_name(hwnd).unwrap_or_default();

        Some(FocusInfo {
            process_id,
            process_name,
            exe_path,
            window_class,
            window_title,
        })
    }
}

pub fn is_workstation_locked() -> bool {
    unsafe {
        let desktop = match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP)
        {
            Ok(desktop) => desktop,
            Err(_) => return true,
        };

        let is_unlocked = SwitchDesktop(desktop).is_ok();
        let _ = CloseDesktop(desktop);
        !is_unlocked
    }
}

fn is_lock_screen_process(process_name: &str) -> bool {
    matches!(
        process_name.to_ascii_lowercase().as_str(),
        "lockapp.exe" | "logonui.exe" | "credentialuibroker.exe"
    )
}

fn window_text(hwnd: HWND) -> Option<String> {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }

        let mut buffer = vec![0_u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buffer);
        wide_slice_to_string(&buffer[..copied.max(0) as usize])
    }
}

fn class_name(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut buffer = vec![0_u16; 256];
        let copied = GetClassNameW(hwnd, &mut buffer);
        wide_slice_to_string(&buffer[..copied.max(0) as usize])
    }
}

fn process_path(process_id: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let mut buffer = vec![0_u16; 32_768];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);

        result
            .ok()
            .and_then(|_| wide_slice_to_string(&buffer[..len as usize]))
    }
}

fn process_name(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn wide_slice_to_string(slice: &[u16]) -> Option<String> {
    let text = String::from_utf16_lossy(slice).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}
