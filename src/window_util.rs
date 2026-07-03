use gpui::{Window, size};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::{
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            EnumWindows, FindWindowExW, FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW,
            HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, LWA_ALPHA, SMTO_NORMAL, SW_SHOWNA,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SendMessageTimeoutW,
            SetLayeredWindowAttributes, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            WS_EX_LAYERED, WS_MAXIMIZEBOX, WS_THICKFRAME,
        },
    },
    core::w,
};

pub fn hwnd_from_window(window: &Window) -> Option<isize> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

pub fn set_topmost(hwnd: isize, topmost: bool) {
    if hwnd == 0 {
        return;
    }

    let insert_after = if topmost {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    unsafe {
        let _ = SetWindowPos(
            HWND(hwnd as _),
            Some(insert_after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn attach_to_desktop(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    unsafe {
        let hwnd = HWND(hwnd as _);
        if let Some(target) = desktop_worker_window() {
            let _ = SetParent(hwnd, Some(target));
        }
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn detach_from_desktop(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    unsafe {
        let _ = SetParent(HWND(hwnd as _), None);
    }
}

pub fn set_window_opacity(hwnd: isize, opacity_percent: u8) {
    if hwnd == 0 {
        return;
    }

    let opacity = opacity_percent.clamp(40, 100);
    let alpha = ((opacity as u16 * 255) / 100) as u8;
    unsafe {
        let hwnd = HWND(hwnd as _);
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
    }
}

pub fn disable_maximize(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    update_window_style(hwnd, |style| style & !(WS_MAXIMIZEBOX.0 as isize));
}

pub fn set_window_resize_enabled(hwnd: isize, enabled: bool) {
    if hwnd == 0 {
        return;
    }

    update_window_style(hwnd, |style| {
        let style = style & !(WS_MAXIMIZEBOX.0 as isize);
        if enabled {
            style | WS_THICKFRAME.0 as isize
        } else {
            style & !(WS_THICKFRAME.0 as isize)
        }
    });
}

fn update_window_style(hwnd: isize, update: impl FnOnce(isize) -> isize) {
    unsafe {
        let hwnd = HWND(hwnd as _);
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        let next_style = update(style);
        if next_style != style {
            let _ = SetWindowLongPtrW(hwnd, GWL_STYLE, next_style);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

pub fn resize_window(window: &mut Window, width: u32, height: u32) {
    window.resize(size(gpui::px(width as f32), gpui::px(height as f32)));
}

fn desktop_worker_window() -> Option<HWND> {
    unsafe {
        let progman = FindWindowW(w!("Progman"), None).ok()?;
        let mut result = 0_usize;
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result),
        );
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0x0D),
            LPARAM(0),
            SMTO_NORMAL,
            1000,
            Some(&mut result),
        );
        let _ = SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0x0D),
            LPARAM(1),
            SMTO_NORMAL,
            1000,
            Some(&mut result),
        );

        let mut worker = HWND::default();
        let worker_ptr = &mut worker as *mut HWND;
        let _ = EnumWindows(Some(enum_desktop_worker), LPARAM(worker_ptr as isize));
        if worker.0.is_null() {
            Some(progman)
        } else {
            Some(worker)
        }
    }
}

unsafe extern "system" fn enum_desktop_worker(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let shell_view = unsafe { FindWindowExW(Some(hwnd), None, w!("SHELLDLL_DefView"), None) };
    if shell_view.is_ok() {
        let target = unsafe { FindWindowExW(None, Some(hwnd), w!("WorkerW"), None) }
            .ok()
            .unwrap_or(hwnd);
        if target != hwnd {
            unsafe {
                let _ = ShowWindow(target, SW_SHOWNA);
            }
        }
        let result = lparam.0 as *mut HWND;
        if !result.is_null() {
            unsafe {
                *result = target;
            }
        }
        return false.into();
    }
    true.into()
}
