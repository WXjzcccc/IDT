use std::{path::Path, thread, time::Duration};

use anyhow::Result;
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM},
        System::Threading::{
            CreateMutexW, GetCurrentProcessId, OpenProcess, PROCESS_NAME_FORMAT,
            PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
            SW_RESTORE, SetForegroundWindow, ShowWindow,
        },
    },
    core::{BOOL, PWSTR, w},
};

const MUTEX_NAME: windows::core::PCWSTR = w!("Local\\IDT.I-Did-Today.SingleInstance");
const WINDOW_TITLE: &str = "I Did Today";

pub struct SingleInstance {
    mutex: HANDLE,
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.mutex);
        }
    }
}

pub fn acquire_or_activate(activate_existing: bool) -> Result<Option<SingleInstance>> {
    let mutex = unsafe { CreateMutexW(None, false, MUTEX_NAME)? };
    let already_running = unsafe { GetLastError() == ERROR_ALREADY_EXISTS };

    if already_running {
        if activate_existing {
            activate_existing_instance();
        }
        unsafe {
            let _ = CloseHandle(mutex);
        }
        return Ok(None);
    }

    Ok(Some(SingleInstance { mutex }))
}

fn activate_existing_instance() -> bool {
    for _ in 0..40 {
        if let Some(hwnd) = find_existing_window() {
            unsafe {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(hwnd);
            }
            return true;
        }

        thread::sleep(Duration::from_millis(50));
    }

    false
}

fn find_existing_window() -> Option<HWND> {
    let current_pid = unsafe { GetCurrentProcessId() };
    let current_process_name = std::env::current_exe()
        .ok()
        .and_then(|path| process_name(path.as_path()))
        .map(|name| name.to_ascii_lowercase());

    let mut state = FindWindowState {
        current_pid,
        current_process_name,
        hwnd: None,
    };

    unsafe {
        let _ = EnumWindows(
            Some(enum_window_for_existing_instance),
            LPARAM((&mut state as *mut FindWindowState) as isize),
        );
    }

    state.hwnd
}

struct FindWindowState {
    current_pid: u32,
    current_process_name: Option<String>,
    hwnd: Option<HWND>,
}

unsafe extern "system" fn enum_window_for_existing_instance(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = unsafe { &mut *(lparam.0 as *mut FindWindowState) };

    let mut process_id = 0_u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    }
    if process_id == 0 || process_id == state.current_pid {
        return BOOL(1);
    }

    if window_text(hwnd).as_deref() != Some(WINDOW_TITLE) {
        return BOOL(1);
    }

    if let Some(current_name) = &state.current_process_name {
        let candidate_name = process_path(process_id)
            .as_deref()
            .and_then(|path| process_name(Path::new(path)))
            .map(|name| name.to_ascii_lowercase());

        if candidate_name.as_deref() != Some(current_name.as_str()) {
            return BOOL(1);
        }
    }

    state.hwnd = Some(hwnd);
    BOOL(0)
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

fn process_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn wide_slice_to_string(slice: &[u16]) -> Option<String> {
    let text = String::from_utf16_lossy(slice).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}
