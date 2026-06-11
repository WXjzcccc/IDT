use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicIsize, Ordering},
    },
    thread,
};

use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        System::Threading::GetCurrentProcess,
        System::{LibraryLoader::GetModuleHandleW, ProcessStatus::EmptyWorkingSet},
        UI::{
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
                Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AdjustWindowRectEx, AppendMenuW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
                CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
                GWL_EXSTYLE, GWL_STYLE, GetCursorPos, GetMessageW, GetWindowLongW, HMENU,
                HWND_MESSAGE, IDI_APPLICATION, IsWindowVisible, LoadIconW, MF_SEPARATOR, MF_STRING,
                MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE, SW_MINIMIZE,
                SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos,
                ShowWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
                WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
                WM_NULL, WM_RBUTTONUP, WNDCLASSW,
            },
        },
    },
    core::w,
};

use crate::{app_icon, db::WindowSize};

const TRAY_UID: u32 = 1;
const TRAY_CALLBACK: u32 = WM_APP + 17;
const MENU_OPEN: usize = 1001;
const MENU_EXIT: usize = 1002;

static TARGET_HWND: OnceLock<Arc<AtomicIsize>> = OnceLock::new();
static EXIT_REQUESTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static SHOW_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn start(target_hwnd: Arc<AtomicIsize>, exit_requested: Arc<AtomicBool>) {
    let _ = TARGET_HWND.set(target_hwnd);
    let _ = EXIT_REQUESTED.set(exit_requested);

    thread::Builder::new()
        .name("idt-system-tray".to_owned())
        .spawn(run_tray_loop)
        .expect("system tray thread should start");
}

pub fn show_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    SHOW_REQUESTED.store(true, Ordering::Relaxed);
    unsafe {
        let hwnd = HWND(hwnd as _);
        let _ = ShowWindow(hwnd, SW_RESTORE);
        let _ = SetForegroundWindow(hwnd);
    }
}

pub fn hide_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    unsafe {
        let _ = ShowWindow(HWND(hwnd as _), SW_HIDE);
    }
}

pub fn is_window_visible(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }

    unsafe { IsWindowVisible(HWND(hwnd as _)).as_bool() }
}

pub fn take_show_requested() -> bool {
    SHOW_REQUESTED.swap(false, Ordering::Relaxed)
}

pub fn trim_working_set() {
    unsafe {
        let _ = EmptyWorkingSet(GetCurrentProcess());
    }
}

pub fn minimize_window(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    unsafe {
        let _ = ShowWindow(HWND(hwnd as _), SW_MINIMIZE);
    }
}

pub fn center_window(hwnd: isize, window_size: WindowSize) {
    if hwnd == 0 {
        return;
    }

    unsafe {
        let hwnd = HWND(hwnd as _);
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            return;
        }

        let mut rect = RECT {
            left: 0,
            top: 0,
            right: window_size.width as i32,
            bottom: window_size.height as i32,
        };
        let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
        let ex_style = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
        if AdjustWindowRectEx(&mut rect, style, false, ex_style).is_err() {
            return;
        }

        let window_width = rect.right - rect.left;
        let window_height = rect.bottom - rect.top;
        let work_area = monitor_info.rcWork;
        let work_width = work_area.right - work_area.left;
        let work_height = work_area.bottom - work_area.top;
        let x = work_area.left + (work_width - window_width).max(0) / 2;
        let y = work_area.top + (work_height - window_height).max(0) / 2;

        let _ = SetWindowPos(
            hwnd,
            None,
            x,
            y,
            window_width,
            window_height,
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

fn run_tray_loop() {
    unsafe {
        let module = match GetModuleHandleW(None) {
            Ok(module) => module,
            Err(error) => {
                eprintln!("failed to get module handle for tray: {error:#}");
                return;
            }
        };

        let window_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(tray_wnd_proc),
            hInstance: module.into(),
            lpszClassName: w!("IDTTrayWindow"),
            ..Default::default()
        };

        RegisterClassW(&window_class);

        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("IDTTrayWindow"),
            w!("I Did Today"),
            WINDOW_STYLE(0),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            0,
            0,
            Some(HWND_MESSAGE),
            Option::<HMENU>::None,
            Some(module.into()),
            None,
        ) {
            Ok(hwnd) => hwnd,
            Err(error) => {
                eprintln!("failed to create tray window: {error:#}");
                return;
            }
        };

        add_tray_icon(hwnd);

        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).into() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        TRAY_CALLBACK => {
            match lparam.0 as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => show_target_window(),
                WM_RBUTTONUP => show_context_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_tray_icon(hwnd);
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn show_target_window() {
    let Some(target) = TARGET_HWND.get() else {
        return;
    };

    show_window(target.load(Ordering::Relaxed));
}

fn request_exit(hwnd: HWND) {
    if let Some(exit_requested) = EXIT_REQUESTED.get() {
        exit_requested.store(true, Ordering::Relaxed);
    }

    unsafe {
        remove_tray_icon(hwnd);
        PostQuitMessage(0);
    }
}

fn show_context_menu(hwnd: HWND) {
    unsafe {
        let menu = match CreatePopupMenu() {
            Ok(menu) => menu,
            Err(error) => {
                eprintln!("failed to create tray menu: {error:#}");
                return;
            }
        };

        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("打开 I Did Today"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("退出"));

        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_ok() {
            let _ = SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                Some(0),
                hwnd,
                None,
            );

            match command.0 as usize {
                MENU_OPEN => show_target_window(),
                MENU_EXIT => request_exit(hwnd),
                _ => {}
            }

            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        }

        let _ = DestroyMenu(menu);
    }
}

fn add_tray_icon(hwnd: HWND) {
    unsafe {
        let data = notify_icon_data(hwnd);
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            eprintln!("failed to add tray icon");
        }
    }
}

fn remove_tray_icon(hwnd: HWND) {
    unsafe {
        let data = notify_icon_data(hwnd);
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

fn notify_icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW::default();
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_UID;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = TRAY_CALLBACK;

    unsafe {
        if let Some(icon) = app_icon::load_icon(32) {
            data.hIcon = icon;
        } else if let Ok(icon) = LoadIconW(None, IDI_APPLICATION) {
            data.hIcon = icon;
        }
    }

    write_wide(&mut data.szTip, "I Did Today");
    data
}

fn write_wide(target: &mut [u16], text: &str) {
    for (slot, value) in target
        .iter_mut()
        .zip(text.encode_utf16().chain(std::iter::once(0)))
    {
        *slot = value;
    }
}
