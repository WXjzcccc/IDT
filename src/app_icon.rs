use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{
        CreateIconFromResourceEx, HICON, ICON_BIG, ICON_SMALL, ICON_SMALL2, IMAGE_FLAGS,
        SendMessageW, WM_SETICON,
    },
};

const ICON_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/idt.ico"));
const ICON_RESOURCE_VERSION: u32 = 0x0003_0000;

pub fn load_icon(preferred_size: u16) -> Option<HICON> {
    let entry = select_icon_entry(preferred_size)?;
    let start = entry.offset as usize;
    let end = start.checked_add(entry.size as usize)?;
    let data = ICON_BYTES.get(start..end)?;

    unsafe {
        CreateIconFromResourceEx(
            data,
            true,
            ICON_RESOURCE_VERSION,
            preferred_size as i32,
            preferred_size as i32,
            IMAGE_FLAGS(0),
        )
        .ok()
    }
}

pub fn apply_window_icons(hwnd: isize) {
    if hwnd == 0 {
        return;
    }

    let hwnd = HWND(hwnd as _);
    unsafe {
        if let Some(icon) = load_icon(16) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL2 as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }

        if let Some(icon) = load_icon(256).or_else(|| load_icon(64)) {
            let _ = SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }
}

#[derive(Clone, Copy)]
struct IconEntry {
    size: u32,
    offset: u32,
    width: u16,
}

fn select_icon_entry(preferred_size: u16) -> Option<IconEntry> {
    if ICON_BYTES.len() < 6 || read_u16(0)? != 0 || read_u16(2)? != 1 {
        return None;
    }

    let count = read_u16(4)? as usize;
    let mut best = None::<IconEntry>;
    for ix in 0..count {
        let offset = 6 + ix * 16;
        let width = match *ICON_BYTES.get(offset)? {
            0 => 256,
            width => width as u16,
        };
        let entry = IconEntry {
            width,
            size: read_u32(offset + 8)?,
            offset: read_u32(offset + 12)?,
        };

        if width == preferred_size {
            return Some(entry);
        }

        best = match best {
            None => Some(entry),
            Some(current)
                if width.abs_diff(preferred_size) < current.width.abs_diff(preferred_size) =>
            {
                Some(entry)
            }
            Some(current) => Some(current),
        };
    }

    best
}

fn read_u16(offset: usize) -> Option<u16> {
    let bytes = ICON_BYTES.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(offset: usize) -> Option<u32> {
    let bytes = ICON_BYTES.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
