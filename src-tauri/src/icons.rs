//! Windows 原生图标提取。
//!
//! `SHGetFileInfoW` 会走 shell 各自的图标处理器，所以：
//! - `.lnk`  → 拿到**快捷方式指向的目标**的图标（不是那个白底箭头）
//! - `.exe`  → 拿到可执行文件内嵌的图标资源
//! - 目录    → 拿到系统文件夹图标
//! - 其他    → 按扩展名关联的图标
//!
//! 一张 32×32 的图标压成 PNG 后约 1KB，转 base64 约 1.4KB。
//! 一次只给当前分类的条目取图标（约 30 张 ≈ 40KB），不做全量预取。
//!
//! 失败一律返回 None —— 取不到图标不该让整个界面出错，前端会退回几何图形。

#![cfg(windows)]

use base64::Engine;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DrawIconEx, HICON, DI_NORMAL,
};

/// 提取出的图标尺寸（系统"大图标"标准尺寸）
const ICON_SIZE: u32 = 32;

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 把 `path` 的图标转成 `data:image/png;base64,...`，失败返回 None。
pub fn icon_data_url(path: &str) -> Option<String> {
    let png = extract_png(path)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Some(format!("data:image/png;base64,{b64}"))
}

fn extract_png(path: &str) -> Option<Vec<u8>> {
    let wide = to_wide(path);
    let mut shfi: SHFILEINFOW = unsafe { std::mem::zeroed() };

    // SHGFI_LARGEICON 要的是 32×32；shell 会替我们解析 .lnk 的目标
    let ok = unsafe {
        SHGetFileInfoW(
            wide.as_ptr(),
            0,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if ok == 0 || shfi.hIcon.is_null() {
        return None;
    }

    let rgba = unsafe { hicon_to_rgba(shfi.hIcon) };
    // HICON 由 shell 分配，必须销毁，否则每次调用泄漏一个 GDI 对象
    unsafe { DestroyIcon(shfi.hIcon) };

    let rgba = rgba?;
    encode_png(&rgba)
}

/// HICON → RGBA 字节（top-down，每像素 4 字节）
unsafe fn hicon_to_rgba(hicon: HICON) -> Option<Vec<u8>> {
    let size = ICON_SIZE;
    let hdc_screen = GetDC(std::ptr::null_mut());
    if hdc_screen.is_null() {
        return None;
    }
    let hdc = CreateCompatibleDC(hdc_screen);
    if hdc.is_null() {
        ReleaseDC(std::ptr::null_mut(), hdc_screen);
        return None;
    }

    // 32bpp top-down DIB。负高度 = top-down，省得后面再翻转。
    let mut bmi: BITMAPINFO = std::mem::zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: size as i32,
        biHeight: -(size as i32),
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..std::mem::zeroed()
    };

    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hbmp = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if hbmp.is_null() || bits.is_null() {
        DeleteDC(hdc);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);
        return None;
    }

    let old = SelectObject(hdc, hbmp);

    let len = (size * size * 4) as usize;
    // 先清零：DrawIconEx 只会覆写图标覆盖到的像素
    let px = std::slice::from_raw_parts_mut(bits as *mut u8, len);
    px.fill(0);

    DrawIconEx(hdc, 0, 0, hicon, size as i32, size as i32, 0, std::ptr::null_mut(), DI_NORMAL);

    let mut out = px.to_vec();

    SelectObject(hdc, old);
    DeleteObject(hbmp);
    DeleteDC(hdc);
    ReleaseDC(std::ptr::null_mut(), hdc_screen);

    // DIB 是 BGRA，浏览器要 RGBA
    for p in out.chunks_exact_mut(4) {
        p.swap(0, 2);
    }

    // 老式图标（无 alpha 通道）画进 32bpp DIB 后 alpha 会全是 0，
    // 结果整个图标变全透明。这种情况把全部像素当作不透明处理。
    if out.chunks_exact(4).all(|p| p[3] == 0) {
        for p in out.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }

    Some(out)
}

fn encode_png(rgba: &[u8]) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut buf, ICON_SIZE, ICON_SIZE);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    Some(buf)
}

/// 批量取图标。前端按当前分类调用，不做全量预取。
pub fn batch(paths: Vec<String>) -> Vec<IconEntry> {
    paths
        .into_iter()
        .map(|path| {
            let data = icon_data_url(&path);
            IconEntry { path, data }
        })
        .collect()
}

#[derive(serde::Serialize)]
pub struct IconEntry {
    pub path: String,
    /// `data:image/png;base64,...`；取不到时为 null
    pub data: Option<String>,
}

#[cfg(not(windows))]
pub fn batch(paths: Vec<String>) -> Vec<IconEntry> {
    paths
        .into_iter()
        .map(|path| IconEntry { path, data: None })
        .collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn assert_valid_png(data: &str) {
        assert!(data.starts_with("data:image/png;base64,"), "应是 PNG data URL");
        let b64 = data.trim_start_matches("data:image/png;base64,");
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).expect("base64 应可解码");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "PNG 魔数不对");
        assert!(bytes.len() > 200, "PNG 太小，可能是空图");
    }

    /// 用系统目录验证整条链路：SHGetFileInfoW → HICON → DIB → PNG。
    /// 不依赖任何用户文件，所以在任何 Windows 上都能跑。
    #[test]
    fn extracts_icon_for_directory() {
        let data = icon_data_url("C:\\Windows").expect("系统目录应该能取到图标");
        assert_valid_png(&data);
    }

    /// 不存在的路径不该 panic，只返回 None（前端会退回几何图形）
    #[test]
    fn missing_path_returns_none_without_panicking() {
        let missing = "C:\\__definitely_not_a_real_path_zzz__\\nope.exe";
        let entries = batch(vec![missing.to_string()]);
        assert_eq!(entries.len(), 1, "即使取不到图标也要为每个路径返回一项");
        assert!(entries[0].data.is_none());
    }

    #[test]
    fn batch_preserves_order_and_count() {
        let paths: Vec<String> = vec!["C:\\Windows".into(), "C:\\nope__zzz".into()];
        let entries = batch(paths.clone());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, paths[0]);
        assert_eq!(entries[1].path, paths[1]);
    }
}
