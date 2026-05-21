use std::{ffi::c_void, io::Cursor, path::Path};

use anyhow::{Context as _, Result, anyhow};
use image::{ColorType, ImageEncoder, codecs::png::PngEncoder};
use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
            DeleteObject, GetDC, HDC, HGDIOBJ, ReleaseDC, SelectObject,
        },
        UI::{
            Shell::ExtractIconExW,
            WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx, HICON},
        },
    },
    core::PCWSTR,
};

const ICON_SIZE: i32 = 64;
const ICON_RETRY_AFTER_MS: i64 = 60 * 60 * 1_000;

#[derive(Debug)]
pub struct ProcessIcon {
    pub png: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

pub fn icon_retry_after_ms() -> i64 {
    ICON_RETRY_AFTER_MS
}

pub fn extract_png_from_exe(path: &str) -> Result<ProcessIcon> {
    if path.trim().is_empty() || !Path::new(path).exists() {
        return Err(anyhow!("process executable path is unavailable"));
    }

    let icon = extract_icon(path)?;
    let result = icon_to_png(icon);
    unsafe {
        let _ = DestroyIcon(icon);
    }
    result
}

fn extract_icon(path: &str) -> Result<HICON> {
    let wide_path = path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut large_icon = HICON::default();
    let mut small_icon = HICON::default();

    let count = unsafe {
        ExtractIconExW(
            PCWSTR(wide_path.as_ptr()),
            0,
            Some(&mut large_icon),
            Some(&mut small_icon),
            1,
        )
    };

    let icon = if !large_icon.is_invalid() {
        if !small_icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(small_icon);
            }
        }
        large_icon
    } else {
        small_icon
    };

    if count == 0 || icon.is_invalid() {
        Err(anyhow!("unable to extract executable icon"))
    } else {
        Ok(icon)
    }
}

fn icon_to_png(icon: HICON) -> Result<ProcessIcon> {
    let screen_dc = unsafe { GetDC(Option::<HWND>::None) };
    if screen_dc.is_invalid() {
        return Err(anyhow!("unable to acquire screen device context"));
    }

    let result = draw_icon_to_png(screen_dc, icon);
    unsafe {
        let _ = ReleaseDC(Option::<HWND>::None, screen_dc);
    }
    result
}

fn draw_icon_to_png(screen_dc: HDC, icon: HICON) -> Result<ProcessIcon> {
    let memory_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    if memory_dc.is_invalid() {
        return Err(anyhow!("unable to create icon device context"));
    }

    let result = render_icon(memory_dc, icon);
    unsafe {
        let _ = DeleteDC(memory_dc);
    }
    result
}

fn render_icon(memory_dc: HDC, icon: HICON) -> Result<ProcessIcon> {
    let mut bitmap_info = BITMAPINFO::default();
    bitmap_info.bmiHeader.biSize =
        size_of::<windows::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
    bitmap_info.bmiHeader.biWidth = ICON_SIZE;
    bitmap_info.bmiHeader.biHeight = -ICON_SIZE;
    bitmap_info.bmiHeader.biPlanes = 1;
    bitmap_info.bmiHeader.biBitCount = 32;
    bitmap_info.bmiHeader.biCompression = BI_RGB.0;

    let mut bits = std::ptr::null_mut::<c_void>();
    let bitmap = unsafe {
        CreateDIBSection(
            Some(memory_dc),
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    }
    .context("unable to create icon bitmap")?;

    if bitmap.is_invalid() || bits.is_null() {
        return Err(anyhow!("unable to allocate icon bitmap"));
    }

    let old_object = unsafe { SelectObject(memory_dc, HGDIOBJ::from(bitmap)) };
    let result = draw_selected_bitmap(memory_dc, icon, bits);

    unsafe {
        if !old_object.is_invalid() {
            let _ = SelectObject(memory_dc, old_object);
        }
        let _ = DeleteObject(HGDIOBJ::from(bitmap));
    }

    result
}

fn draw_selected_bitmap(memory_dc: HDC, icon: HICON, bits: *mut c_void) -> Result<ProcessIcon> {
    unsafe {
        DrawIconEx(
            memory_dc, 0, 0, icon, ICON_SIZE, ICON_SIZE, 0, None, DI_NORMAL,
        )
    }
    .context("unable to draw executable icon")?;

    let byte_len = (ICON_SIZE * ICON_SIZE * 4) as usize;
    let bgra = unsafe { std::slice::from_raw_parts(bits as *const u8, byte_len) };
    let mut rgba = Vec::with_capacity(byte_len);
    let mut has_alpha = false;

    for pixel in bgra.chunks_exact(4) {
        rgba.push(pixel[2]);
        rgba.push(pixel[1]);
        rgba.push(pixel[0]);
        rgba.push(pixel[3]);
        has_alpha |= pixel[3] != 0;
    }

    if !has_alpha {
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = if pixel[0] == 0 && pixel[1] == 0 && pixel[2] == 0 {
                0
            } else {
                255
            };
        }
    }

    let mut png = Cursor::new(Vec::new());
    PngEncoder::new(&mut png)
        .write_image(
            &rgba,
            ICON_SIZE as u32,
            ICON_SIZE as u32,
            ColorType::Rgba8.into(),
        )
        .context("unable to encode executable icon")?;

    Ok(ProcessIcon {
        png: png.into_inner(),
        width: ICON_SIZE,
        height: ICON_SIZE,
    })
}
