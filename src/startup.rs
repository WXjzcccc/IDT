use anyhow::{Result, anyhow};
use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
            REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
            RegSetValueExW,
        },
    },
    core::{PCWSTR, w},
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("I Did Today");
const LEGACY_VALUE_NAME: PCWSTR = w!("IDT");
const STARTUP_ARG: &str = "--startup";
const SILENT_ARG: &str = "--silent";

pub fn is_silent_launch() -> bool {
    std::env::args().any(|arg| arg.eq_ignore_ascii_case(SILENT_ARG))
}

pub fn is_enabled() -> Result<bool> {
    let Some(key) = open_run_key(KEY_QUERY_VALUE)? else {
        return Ok(false);
    };

    let result = match startup_value_exists(key, VALUE_NAME) {
        Ok(true) => Ok(true),
        Ok(false) => startup_value_exists(key, LEGACY_VALUE_NAME),
        Err(error) => Err(error),
    };
    close_key(key);
    result
}

fn startup_value_exists(key: HKEY, value_name: PCWSTR) -> Result<bool> {
    let mut value_type = REG_SZ;
    let mut data_len = 0_u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            value_name,
            None,
            Some(&mut value_type),
            None,
            Some(&mut data_len),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    win32_ok(status, "读取开机自启设置失败")?;
    Ok(data_len > 0)
}

pub fn set_enabled(enabled: bool, silent_start: bool) -> Result<()> {
    let key = create_run_key()?;
    let result = if enabled {
        let command = startup_command(silent_start)?;
        let bytes = wide_bytes(&command);
        let status = unsafe { RegSetValueExW(key, VALUE_NAME, None, REG_SZ, Some(&bytes)) };
        win32_ok(status, "保存开机自启设置失败")
            .and_then(|_| delete_value_if_exists(key, LEGACY_VALUE_NAME))
    } else {
        delete_value_if_exists(key, VALUE_NAME)
            .and_then(|_| delete_value_if_exists(key, LEGACY_VALUE_NAME))
    };
    close_key(key);
    result
}

fn startup_command(silent_start: bool) -> Result<String> {
    let exe = std::env::current_exe()?;
    let exe = exe.display().to_string();
    let command = format!("\"{exe}\" {STARTUP_ARG}");
    if silent_start {
        Ok(format!("{command} {SILENT_ARG}"))
    } else {
        Ok(command)
    }
}

fn open_run_key(access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Result<Option<HKEY>> {
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, access, &mut key) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    win32_ok(status, "打开开机自启注册表失败")?;
    Ok(Some(key))
}

fn create_run_key() -> Result<HKEY> {
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_QUERY_VALUE,
            None,
            &mut key,
            None,
        )
    };
    win32_ok(status, "创建开机自启注册表失败")?;
    Ok(key)
}

fn close_key(key: HKEY) {
    unsafe {
        let _ = RegCloseKey(key);
    }
}

fn delete_value_if_exists(key: HKEY, value_name: PCWSTR) -> Result<()> {
    let status = unsafe { RegDeleteValueW(key, value_name) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    win32_ok(status, "保存开机自启设置失败")
}

fn wide_bytes(value: &str) -> Vec<u8> {
    let wide = value
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let byte_len = wide.len() * size_of::<u16>();
    unsafe { std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), byte_len).to_vec() }
}

fn win32_ok(status: WIN32_ERROR, context: &'static str) -> Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(anyhow!("{context}: {}", status.0))
    }
}
