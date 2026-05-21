use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/idt-icon.svg");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let icon_path = out_dir.join("idt.ico");
    fs::write(&icon_path, build_ico()).expect("write generated IDT icon");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let rc_path = out_dir.join("idt.rc");
        fs::write(&rc_path, resource_script(&icon_path))
            .expect("write generated IDT resource script");
        let _ = embed_resource::compile(rc_path, embed_resource::NONE);
    }
}

fn resource_script(icon_path: &std::path::Path) -> String {
    let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_owned());
    let file_version = rc_version(&version);

    format!(
        r#"1 ICON "{}"

1 VERSIONINFO
FILEVERSION {file_version}
PRODUCTVERSION {file_version}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "I Did Today\0"
            VALUE "FileDescription", "I Did Today\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "InternalName", "IDT\0"
            VALUE "OriginalFilename", "IDT.exe\0"
            VALUE "ProductName", "I Did Today\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#,
        rc_path_string(icon_path)
    )
}

fn rc_version(version: &str) -> String {
    let mut parts = version
        .split('.')
        .take(4)
        .map(|part| part.parse::<u16>().unwrap_or(0))
        .collect::<Vec<_>>();
    while parts.len() < 4 {
        parts.push(0);
    }

    parts
        .into_iter()
        .map(|part| part.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Clone, Copy)]
struct Rgba {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Rgba {
    const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: r as f32,
            g: g as f32,
            b: b as f32,
            a: 255.0,
        }
    }

    const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32,
            g: g as f32,
            b: b as f32,
            a: a as f32,
        }
    }
}

fn build_ico() -> Vec<u8> {
    let sizes = [16_u32, 24, 32, 48, 64, 128, 256];
    let images = sizes
        .iter()
        .map(|size| icon_image(*size))
        .collect::<Vec<_>>();

    let mut ico = Vec::new();
    push_u16(&mut ico, 0);
    push_u16(&mut ico, 1);
    push_u16(&mut ico, sizes.len() as u16);

    let image_offset = 6 + sizes.len() * 16;
    let mut offset = image_offset as u32;
    for (size, image) in sizes.iter().zip(images.iter()) {
        ico.push(if *size == 256 { 0 } else { *size as u8 });
        ico.push(if *size == 256 { 0 } else { *size as u8 });
        ico.push(0);
        ico.push(0);
        push_u16(&mut ico, 1);
        push_u16(&mut ico, 32);
        push_u32(&mut ico, image.len() as u32);
        push_u32(&mut ico, offset);
        offset += image.len() as u32;
    }

    for image in images {
        ico.extend(image);
    }
    ico
}

fn icon_image(size: u32) -> Vec<u8> {
    let rgba = render_icon(size);
    let mask_stride = size.div_ceil(32) * 4;
    let mut image = Vec::new();

    push_u32(&mut image, 40);
    push_i32(&mut image, size as i32);
    push_i32(&mut image, (size * 2) as i32);
    push_u16(&mut image, 1);
    push_u16(&mut image, 32);
    push_u32(&mut image, 0);
    push_u32(&mut image, size * size * 4);
    push_i32(&mut image, 0);
    push_i32(&mut image, 0);
    push_u32(&mut image, 0);
    push_u32(&mut image, 0);

    for y in (0..size).rev() {
        for x in 0..size {
            let ix = ((y * size + x) * 4) as usize;
            image.push(rgba[ix + 2]);
            image.push(rgba[ix + 1]);
            image.push(rgba[ix]);
            image.push(rgba[ix + 3]);
        }
    }

    image.extend(std::iter::repeat_n(0, (mask_stride * size) as usize));
    image
}

fn render_icon(size: u32) -> Vec<u8> {
    let scale = if size <= 24 { 8 } else { 4 };
    let canvas = size * scale;
    let mut pixels = vec![[0.0_f32; 4]; (canvas * canvas) as usize];
    let s = canvas as f32;

    round_rect(
        &mut pixels,
        canvas,
        0.05 * s,
        0.05 * s,
        0.9 * s,
        0.9 * s,
        0.22 * s,
        Rgba::rgb(21, 23, 28),
    );
    round_rect(
        &mut pixels,
        canvas,
        0.09 * s,
        0.08 * s,
        0.82 * s,
        0.34 * s,
        0.17 * s,
        Rgba::rgba(34, 42, 54, 132),
    );

    round_rect(
        &mut pixels,
        canvas,
        0.252 * s,
        0.223 * s,
        0.09 * s,
        0.554 * s,
        0.045 * s,
        Rgba::rgb(247, 250, 255),
    );
    round_rect(
        &mut pixels,
        canvas,
        0.217 * s,
        0.209 * s,
        0.16 * s,
        0.07 * s,
        0.035 * s,
        Rgba::rgb(247, 250, 255),
    );
    round_rect(
        &mut pixels,
        canvas,
        0.217 * s,
        0.721 * s,
        0.16 * s,
        0.07 * s,
        0.035 * s,
        Rgba::rgb(247, 250, 255),
    );

    round_rect(
        &mut pixels,
        canvas,
        0.443 * s,
        0.25 * s,
        0.027 * s,
        0.508 * s,
        0.014 * s,
        Rgba::rgba(247, 250, 255, 116),
    );
    circle(
        &mut pixels,
        canvas,
        0.457 * s,
        0.322 * s,
        0.033 * s,
        Rgba::rgb(59, 130, 246),
    );
    circle(
        &mut pixels,
        canvas,
        0.457 * s,
        0.5 * s,
        0.033 * s,
        Rgba::rgb(34, 197, 94),
    );
    circle(
        &mut pixels,
        canvas,
        0.457 * s,
        0.678 * s,
        0.033 * s,
        Rgba::rgb(244, 176, 0),
    );

    round_rect(
        &mut pixels,
        canvas,
        0.523 * s,
        0.281 * s,
        0.293 * s,
        0.08 * s,
        0.04 * s,
        Rgba::rgb(59, 130, 246),
    );
    round_rect(
        &mut pixels,
        canvas,
        0.523 * s,
        0.459 * s,
        0.381 * s,
        0.08 * s,
        0.04 * s,
        Rgba::rgb(34, 197, 94),
    );
    round_rect(
        &mut pixels,
        canvas,
        0.523 * s,
        0.637 * s,
        0.248 * s,
        0.08 * s,
        0.04 * s,
        Rgba::rgb(244, 176, 0),
    );

    downsample(&pixels, size, scale)
}

fn round_rect(
    pixels: &mut [[f32; 4]],
    width: u32,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: Rgba,
) {
    let radius = radius.min(w * 0.5).min(h * 0.5);
    let min_x = x.floor().max(0.0) as u32;
    let min_y = y.floor().max(0.0) as u32;
    let max_x = (x + w).ceil().min(width as f32) as u32;
    let max_y = (y + h).ceil().min(width as f32) as u32;

    for py in min_y..max_y {
        for px in min_x..max_x {
            let cx = px as f32 + 0.5;
            let cy = py as f32 + 0.5;
            let nearest_x = clamp_or_center(cx, x + radius, x + w - radius);
            let nearest_y = clamp_or_center(cy, y + radius, y + h - radius);
            let dx = cx - nearest_x;
            let dy = cy - nearest_y;
            if dx * dx + dy * dy <= radius * radius {
                blend(&mut pixels[(py * width + px) as usize], color);
            }
        }
    }
}

fn circle(pixels: &mut [[f32; 4]], width: u32, x: f32, y: f32, radius: f32, color: Rgba) {
    let min_x = (x - radius).floor().max(0.0) as u32;
    let min_y = (y - radius).floor().max(0.0) as u32;
    let max_x = (x + radius).ceil().min(width as f32) as u32;
    let max_y = (y + radius).ceil().min(width as f32) as u32;
    let radius_sq = radius * radius;

    for py in min_y..max_y {
        for px in min_x..max_x {
            let dx = px as f32 + 0.5 - x;
            let dy = py as f32 + 0.5 - y;
            if dx * dx + dy * dy <= radius_sq {
                blend(&mut pixels[(py * width + px) as usize], color);
            }
        }
    }
}

fn blend(dst: &mut [f32; 4], src: Rgba) {
    let src_a = src.a / 255.0;
    let dst_a = dst[3] / 255.0;
    let out_a = src_a + dst_a * (1.0 - src_a);
    if out_a <= f32::EPSILON {
        *dst = [0.0; 4];
        return;
    }

    dst[0] = (src.r * src_a + dst[0] * dst_a * (1.0 - src_a)) / out_a;
    dst[1] = (src.g * src_a + dst[1] * dst_a * (1.0 - src_a)) / out_a;
    dst[2] = (src.b * src_a + dst[2] * dst_a * (1.0 - src_a)) / out_a;
    dst[3] = out_a * 255.0;
}

fn clamp_or_center(value: f32, min: f32, max: f32) -> f32 {
    if min <= max {
        value.clamp(min, max)
    } else {
        (min + max) * 0.5
    }
}

fn downsample(pixels: &[[f32; 4]], size: u32, scale: u32) -> Vec<u8> {
    let canvas = size * scale;
    let mut out = vec![0_u8; (size * size * 4) as usize];

    for y in 0..size {
        for x in 0..size {
            let mut alpha = 0.0;
            let mut red = 0.0;
            let mut green = 0.0;
            let mut blue = 0.0;

            for sy in 0..scale {
                for sx in 0..scale {
                    let pixel = pixels[((y * scale + sy) * canvas + (x * scale + sx)) as usize];
                    let a = pixel[3] / 255.0;
                    alpha += a;
                    red += pixel[0] * a;
                    green += pixel[1] * a;
                    blue += pixel[2] * a;
                }
            }

            let samples = (scale * scale) as f32;
            let out_a = alpha / samples;
            let ix = ((y * size + x) * 4) as usize;
            if out_a > f32::EPSILON {
                out[ix] = (red / alpha).round().clamp(0.0, 255.0) as u8;
                out[ix + 1] = (green / alpha).round().clamp(0.0, 255.0) as u8;
                out[ix + 2] = (blue / alpha).round().clamp(0.0, 255.0) as u8;
                out[ix + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    out
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend(value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend(value.to_le_bytes());
}

fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend(value.to_le_bytes());
}

fn rc_path_string(path: &std::path::Path) -> String {
    path.display().to_string().replace('\\', "\\\\")
}
