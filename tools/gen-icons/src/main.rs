//! 品牌资源生成：从 docs/ 下的 SVG 母版渲染应用所需的全部位图资产。
//!
//! 用法（仓库根目录）：cargo run --manifest-path tools/gen-icons/Cargo.toml
//!
//! 产物：
//! - crates/app/assets/icons/app.ico  多尺寸应用图标（exe 嵌入）
//! - crates/app/assets/tray.png       托盘图标（32px，favicon 变体）
//! - docs/banner.png                  README 横幅（logo 2x 渲染）
//!
//! 多尺寸 ico 的策略与专业图标集一致：大尺寸用 icon.svg（细节多），
//! 小尺寸用 favicon.svg（路径简化、特征放大），16px 下仍然可辨。

use std::{fs, path::Path};

use resvg::{tiny_skia, usvg};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").leak()
}

fn render(svg: &str, width: u32, height: u32) -> Vec<u8> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &options).expect("解析 SVG 失败");

    let size = tree.size();
    let (sx, sy) = (
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).expect("创建画布失败");
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
    pixmap.encode_png().expect("编码 PNG 失败")
}

/// 正方形渲染（图标类，viewBox 本身等比）。
fn render_square(svg: &str, size: u32) -> Vec<u8> {
    render(svg, size, size)
}

/// 按宽度等比渲染（横幅类）。
fn render_wide(svg: &str, width: u32) -> Vec<u8> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &options).expect("解析 SVG 失败");
    let size = tree.size();
    let height = (width as f32 * size.height() / size.width()).round() as u32;
    render(svg, width, height)
}

/// 多尺寸 ICO 容器：PNG 帧直排（Vista+ 官方支持，现代应用通行做法）。
/// 头 6 字节 + 每帧 16 字节目录项（宽高各 1B、色数 1B、保留 1B、
/// 平面数 WORD、位深 WORD、数据长度 DWORD、偏移 DWORD）。
fn write_ico(frames: &[(u32, Vec<u8>)], out: &Path) {
    let mut data = Vec::with_capacity(6 + frames.len() * 16);
    data.extend_from_slice(&0u16.to_le_bytes()); // 保留位
    data.extend_from_slice(&1u16.to_le_bytes()); // 类型 1 = 图标
    data.extend_from_slice(&(frames.len() as u16).to_le_bytes());

    let mut offset = 6 + frames.len() * 16;
    for (size, png) in frames.iter() {
        let s = (*size).min(256) as u8; // 256 记作 0
        data.extend_from_slice(&[s, s, 0, 0]);
        data.extend_from_slice(&1u16.to_le_bytes()); // 平面数
        data.extend_from_slice(&32u16.to_le_bytes()); // 位深
        data.extend_from_slice(&((png.len() as u32).to_le_bytes()));
        data.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in frames.iter() {
        data.extend_from_slice(png);
    }
    fs::write(out, data).expect("写入 ico 失败");
}

fn main() {
    let root = repo_root();
    let icon_svg = fs::read_to_string(root.join("docs/icon.svg")).expect("读取 icon.svg 失败");
    let favicon_svg =
        fs::read_to_string(root.join("docs/favicon.svg")).expect("读取 favicon.svg 失败");
    let logo_svg = fs::read_to_string(root.join("docs/logo.svg")).expect("读取 logo.svg 失败");

    let icons_dir = root.join("crates/app/assets/icons");
    fs::create_dir_all(&icons_dir).expect("创建目录失败");

    // 大尺寸走 icon.svg，小尺寸走 favicon.svg（小尺寸优化变体）
    let mut frames = Vec::new();
    for size in [256, 128, 64, 48] {
        frames.push((size, render_square(&icon_svg, size)));
    }
    for size in [32, 24, 16] {
        frames.push((size, render_square(&favicon_svg, size)));
    }
    write_ico(&frames, &icons_dir.join("app.ico"));
    println!("生成 {}", icons_dir.join("app.ico").display());

    let tray = render_square(&favicon_svg, 32);
    fs::write(root.join("crates/app/assets/tray.png"), &tray).expect("写入 tray.png 失败");
    println!("生成 crates/app/assets/tray.png");

    let banner = render_wide(&logo_svg, 1280);
    fs::write(root.join("docs/banner.png"), &banner).expect("写入 banner.png 失败");
    println!("生成 docs/banner.png");
}
