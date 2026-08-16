//! 开发调试：验证图像条目的写回链路（读文件 → 裸 Win32 写入 → 回读）。
//! cargo run -p wisp-core --example clipboard_probe

use std::path::PathBuf;

fn main() {
    let db = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Wisp")
        .join("wisp.db");

    let conn = rusqlite::Connection::open(&db).expect("打开数据库失败");
    let (id, content): (i64, String) = conn
        .query_row(
            "SELECT id, content FROM clips WHERE kind = 1 ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("库中还没有图像条目");

    println!("最新图像条目 #{id} -> {content}");

    // 走产品同款写回路径
    let png = std::fs::read(&content).expect("读取图像文件失败");
    println!("文件 {} 字节", png.len());
    wisp_core::write_image_png(&png).expect("write_image_png 失败");
    println!("write_image_png 成功");

    // 回读验证（arboard 读路径正常）
    let mut clipboard = arboard::Clipboard::new().expect("回读时打开剪贴板失败");
    let back = clipboard.get_image().expect("回读图像失败");
    println!(
        "回读 {}×{}，首像素 RGBA {:?}",
        back.width,
        back.height,
        &back.bytes[..4]
    );
    let text = clipboard.get_text();
    println!("同剪贴板 get_text: {:?}", text.err().map(|e| e.to_string()));
}
