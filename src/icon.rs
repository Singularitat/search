use eframe::egui::{self, ColorImage, ImageData, TextureHandle, TextureOptions};
use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::Path};
use windows::{
    core::PCWSTR,
    Win32::{
        Graphics::Gdi::{
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
            BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
        },
        Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
        UI::{
            Shell::{
                SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES,
            },
            WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO},
        },
    },
};

pub unsafe fn fetch_and_convert_icon(
    ctx: &egui::Context,
    path: &Path,
    attribute_flag: u32, // use FILE_ATTRIBUTE_DIRECTORY or FILE_ATTRIBUTE_NORMAL
) -> Option<TextureHandle> {
    let mut path_utf16: Vec<u16> = path.as_os_str().encode_wide().collect();
    path_utf16.push(0); // null-terminate
    let path_pcwstr = PCWSTR::from_raw(path_utf16.as_ptr());

    let mut shfi: SHFILEINFOW = std::mem::zeroed();
    // use SHGFI_USEFILEATTRIBUTES so Windows doesn't need to access the file/dir itself
    let flags = SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES;

    SHGetFileInfoW(
        path_pcwstr,
        FILE_FLAGS_AND_ATTRIBUTES(attribute_flag), // Use the passed attribute flag
        Some(&mut shfi),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        flags,
    );

    if shfi.hIcon.is_invalid() {
        return None;
    }
    let h_icon: HICON = shfi.hIcon;

    let mut icon_info: ICONINFO = std::mem::zeroed();
    if GetIconInfo(h_icon, &mut icon_info).is_err() {
        let _ = DestroyIcon(h_icon);
        return None;
    }
    // need to clean up icon_info.hbmColor and icon_info.hbmMask later

    let h_bitmap: HBITMAP = icon_info.hbmColor;
    if h_bitmap.is_invalid() {
        if !icon_info.hbmMask.is_invalid() {
            let _ = DeleteObject(icon_info.hbmMask.into());
        }
        let _ = DestroyIcon(h_icon);
        return None;
    }

    let mut bmp: BITMAP = std::mem::zeroed();
    let obj_size = std::mem::size_of::<BITMAP>() as i32;
    if GetObjectW(
        h_bitmap.into(),
        obj_size,
        Some((&raw mut bmp).cast::<std::ffi::c_void>()),
    ) == 0
    {
        let _ = DeleteObject(h_bitmap.into());
        if !icon_info.hbmMask.is_invalid() {
            let _ = DeleteObject(icon_info.hbmMask.into());
        }
        let _ = DestroyIcon(h_icon);
        return None;
    }

    let width = bmp.bmWidth as usize;
    let height = bmp.bmHeight as usize;
    if width == 0 || height == 0 || width > 128 || height > 128 {
        // basic validation
        let _ = DeleteObject(h_bitmap.into());
        if !icon_info.hbmMask.is_invalid() {
            let _ = DeleteObject(icon_info.hbmMask.into());
        }
        let _ = DestroyIcon(h_icon);
        return None;
    }

    let mut pixels_bgra: Vec<u8> = vec![0; width * height * 4];
    let mut bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bmp.bmWidth,
            biHeight: -bmp.bmHeight, // Top-down DIB
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..std::mem::zeroed()
        },
        ..std::mem::zeroed()
    };

    let hdc_screen = GetDC(None);
    if hdc_screen.is_invalid() {
        // clean up allocated resources before returning
        let _ = DeleteObject(h_bitmap.into());
        if !icon_info.hbmMask.is_invalid() {
            let _ = DeleteObject(icon_info.hbmMask.into());
        }
        let _ = DestroyIcon(h_icon);
        return None;
    }

    let result = GetDIBits(
        hdc_screen,
        h_bitmap,
        0,
        height as u32,
        Some(pixels_bgra.as_mut_ptr().cast::<std::ffi::c_void>()),
        &mut bitmap_info,
        DIB_RGB_COLORS,
    );

    let _ = ReleaseDC(None, hdc_screen); // Release DC *after* use

    // delete GDI objects obtained from GetIconInfo before destroying the icon
    let _ = DeleteObject(h_bitmap.into()); // hbmColor
    if !icon_info.hbmMask.is_invalid() {
        let _ = DeleteObject(icon_info.hbmMask.into()); // hbmMask
    }
    // destroy the icon obtained from SHGetFileInfoW
    let _ = DestroyIcon(h_icon);

    if result == 0 {
        return None; // GetDIBits failed
    }

    // convert BGRA to RGBA Vec<Color32>
    let pixels_rgba: Vec<egui::Color32> = pixels_bgra
        .chunks_exact(4)
        .map(|bgra| egui::Color32::from_rgba_unmultiplied(bgra[2], bgra[1], bgra[0], bgra[3])) // Use unmultiplied
        .collect();

    if pixels_rgba.len() != width * height {
        return None; // should not happen if GetDIBits succeeded
    }

    let color_image = ColorImage {
        size: [width, height],
        pixels: pixels_rgba,
    };

    let texture_name = format!(
        "icon_{}",
        path.extension()
            .and_then(OsStr::to_str)
            .map_or_else(|| "<NO_EXT>".to_string(), str::to_lowercase)
    );

    let handle = ctx.load_texture(
        texture_name,
        ImageData::Color(color_image.into()), // Use ImageData enum
        TextureOptions::LINEAR,               // Use enum variant
    );

    Some(handle)
}
