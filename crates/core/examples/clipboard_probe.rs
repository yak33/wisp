//! 开发调试：验证图像与文件条目的写回链路（读库 → 裸 Win32 写入 → 回读）。
//! `cargo run -p wisp-core --example clipboard_probe`

use std::path::PathBuf;

fn main() {
    let db = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Wisp")
        .join("wisp.db");

    let conn = rusqlite::Connection::open(&db).expect("打开数据库失败");

    // 图像：读文件 → 裸 Win32 双格式写入 → arboard 回读
    if let Ok((id, content)) = conn.query_row(
        "SELECT id, content FROM clips WHERE kind = 1 ORDER BY id DESC LIMIT 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    ) {
        println!("最新图像条目 #{id} -> {content}");
        let png = std::fs::read(&content).expect("读取图像文件失败");
        println!("文件 {} 字节", png.len());
        wisp_core::write_image_png(&png).expect("write_image_png 失败");
        println!("write_image_png 成功");

        let mut clipboard = arboard::Clipboard::new().expect("回读时打开剪贴板失败");
        let back = clipboard.get_image().expect("回读图像失败");
        println!(
            "回读 {}×{}，首像素 RGBA {:?}",
            back.width,
            back.height,
            &back.bytes[..4]
        );
    } else {
        println!("（库中还没有图像条目，跳过图像分支）");
    }

    println!("---");

    // 文件：路径清单 → CF_HDROP 裸写入 → arboard 回读
    if let Ok((id, content)) = conn.query_row(
        "SELECT id, content FROM clips WHERE kind = 2 ORDER BY id DESC LIMIT 1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
    ) {
        println!("最新文件条目 #{id} -> {content:?}");
        let paths: Vec<PathBuf> = content.lines().map(PathBuf::from).collect();
        wisp_core::write_files(&paths).expect("write_files 失败");
        println!("write_files 成功");

        let mut clipboard = arboard::Clipboard::new().expect("回读时打开剪贴板失败");
        let back = clipboard.get().file_list().expect("回读文件列表失败");
        println!("回读 {} 个文件:", back.len());
        for path in back.iter().take(5) {
            println!("  {}", path.display());
        }
    } else {
        println!("（库中还没有文件条目，跳过文件分支）");
    }
}
