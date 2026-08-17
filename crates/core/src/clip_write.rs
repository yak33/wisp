//! 图像写回剪贴板：裸 Win32 实现。
//!
//! arboard 3.6.1 的 Windows 图像写入路径在本机持续报
//! `SetClipboardData` os error 1418（线程没有打开的剪贴板），
//! 而同样的裸 Win32 序列完全正常——故绕开它自写。
//!
//! 双格式写入（PNG 注册格式 + CF_DIBV5），顺序与兼容性策略
//! 与 Windows 剪贴板惯例一致：PNG 在前（现代应用优先取，无损），
//! DIBV5 兜底（只认标准位图格式的旧应用）。

use anyhow::{Context as _, Result};
use windows::{
    Win32::{
        Foundation::{GetLastError, HANDLE, WIN32_ERROR},
        Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW,
                SetClipboardData,
            },
            Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{CF_DIBV5, CF_HDROP},
        },
        UI::Shell::DROPFILES,
    },
    core::{BOOL, PCWSTR},
};

/// 注册的 "PNG" 剪贴板格式 ID（进程内不变，注册一次缓存）
static PNG_FORMAT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// 把文件列表写回剪贴板（CF_HDROP）：DROPFILES 头 + 宽字符路径列表，
/// 每个路径 NUL 结尾、整体再补一个 NUL。与图像写入同一重试框架。
pub fn write_files(paths: &[std::path::PathBuf]) -> Result<()> {
    if paths.is_empty() {
        anyhow::bail!("文件列表为空");
    }

    let mut wide: Vec<u16> = Vec::new();
    for path in paths {
        wide.extend(path.as_os_str().to_string_lossy().encode_utf16());
        wide.push(0);
    }
    wide.push(0); // 列表整体终止符

    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        pt: Default::default(),
        fNC: BOOL(0),
        fWide: BOOL(1),
    };
    let mut blob = Vec::with_capacity(std::mem::size_of::<DROPFILES>() + wide.len() * 2);
    let header_bytes = unsafe {
        std::slice::from_raw_parts(
            (&header as *const DROPFILES).cast::<u8>(),
            std::mem::size_of::<DROPFILES>(),
        )
    };
    blob.extend_from_slice(header_bytes);
    blob.extend_from_slice(unsafe {
        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2)
    });

    let mut last: Option<WIN32_ERROR> = None;
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let result = unsafe {
            if OpenClipboard(None).is_err() {
                Err(GetLastError())
            } else {
                let inner = (|| {
                    EmptyClipboard().map_err(|_| GetLastError())?;
                    set_format(CF_HDROP.0 as u32, &blob)
                })();
                _ = CloseClipboard();
                inner
            }
        };
        match result {
            Ok(()) => return Ok(()),
            Err(err) => last = Some(err),
        }
    }
    let code = last.map(|err| err.0).unwrap_or_default();
    Err(anyhow::anyhow!(
        "写入文件列表到剪贴板失败（已重试 5 次，最后错误码 {code}）"
    ))
}

/// 把 PNG 字节写回剪贴板。`png` 为原始文件字节（无需解码重编码，
/// PNG 格式直接透传）；DIBV5 分支内部解码一次转 BGRA。
/// 整个 open→set→close 序列退避重试，应对剪贴板监听方的竞争。
pub fn write_image_png(png: &[u8]) -> Result<()> {
    let decoded = image::load_from_memory(png).context("解码图像失败")?;
    let rgba = decoded.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let bgra_bottom_up = to_bgra_bottom_up(rgba);
    write_image_raw(png, width, height, &bgra_bottom_up)
}

fn write_image_raw(png: &[u8], width: u32, height: u32, bgra: &[u8]) -> Result<()> {
    let mut last: Option<WIN32_ERROR> = None;
    for attempt in 0..5 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        match try_write(png, width, height, bgra) {
            Ok(()) => return Ok(()),
            Err(err) => last = Some(err),
        }
    }
    let code = last.map(|err| err.0).unwrap_or_default();
    Err(anyhow::anyhow!(
        "写入图像到剪贴板失败（已重试 5 次，最后错误码 {code}）"
    ))
}

unsafe fn global_with_data(bytes: &[u8]) -> Result<HANDLE> {
    unsafe {
        let handle = GlobalAlloc(GHND, bytes.len()).context("GlobalAlloc 失败")?;
        let dst = GlobalLock(handle) as *mut u8;
        if dst.is_null() {
            return Err(anyhow::anyhow!("GlobalLock 失败"));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
        _ = GlobalUnlock(handle);
        Ok(HANDLE(handle.0))
    }
}

fn set_format(format: u32, bytes: &[u8]) -> std::result::Result<(), WIN32_ERROR> {
    let handle = match unsafe { global_with_data(bytes) } {
        Ok(handle) => handle,
        Err(_) => return Err(WIN32_ERROR(0)),
    };
    let result = unsafe { SetClipboardData(format, Some(handle)) };
    match result {
        Ok(_) => Ok(()),
        Err(_) => Err(unsafe { GetLastError() }),
    }
}

fn try_write(
    png: &[u8],
    width: u32,
    height: u32,
    bgra: &[u8],
) -> std::result::Result<(), WIN32_ERROR> {
    unsafe {
        if OpenClipboard(None).is_err() {
            return Err(GetLastError());
        }
        // open 成功后的任何退出都必须关剪贴板
        let result = (|| {
            EmptyClipboard().map_err(|_| GetLastError())?;

            // 1) PNG 注册格式：原始字节透传
            let format = *PNG_FORMAT.get_or_init(|| {
                let name: Vec<u16> = "PNG".encode_utf16().chain(std::iter::once(0)).collect();
                RegisterClipboardFormatW(PCWSTR(name.as_ptr()))
            });
            if format == 0 {
                return Err(GetLastError());
            }
            set_format(format, png)?;

            // 2) CF_DIBV5：BITMAPV5HEADER + BGRA（自底向上行序，正高度）
            let mut dib = Vec::with_capacity(std::mem::size_of::<BITMAPV5HEADER>() + bgra.len());
            let header = BITMAPV5HEADER {
                bV5Size: std::mem::size_of::<BITMAPV5HEADER>() as u32,
                bV5Width: width as i32,
                bV5Height: height as i32,
                bV5Planes: 1,
                bV5BitCount: 32,
                bV5Compression: BI_BITFIELDS,
                bV5SizeImage: bgra.len() as u32,
                bV5RedMask: 0x00ff_0000,
                bV5GreenMask: 0x0000_ff00,
                bV5BlueMask: 0x0000_00ff,
                bV5AlphaMask: 0xff00_0000,
                bV5CSType: 0x7352_4742, // 'sRGB'
                ..Default::default()
            };
            let header_bytes = std::slice::from_raw_parts(
                (&header as *const BITMAPV5HEADER).cast::<u8>(),
                std::mem::size_of::<BITMAPV5HEADER>(),
            );
            dib.extend_from_slice(header_bytes);
            dib.extend_from_slice(bgra);
            set_format(CF_DIBV5.0 as u32, &dib)
        })();
        _ = CloseClipboard();
        result
    }
}

/// RGBA（自顶向下）→ BGRA（自底向上）：DIB 正高度行序所需。
fn to_bgra_bottom_up(rgba: image::RgbaImage) -> Vec<u8> {
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    let mut src = rgba.into_raw();
    for pixel in src.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    let mut dst = vec![0u8; src.len()];
    for row in 0..height {
        let src_start = row * width * 4;
        let dst_start = (height - 1 - row) * width * 4;
        dst[dst_start..dst_start + width * 4]
            .copy_from_slice(&src[src_start..src_start + width * 4]);
    }
    dst
}
