//! 开发调试：窥探 wisp.db 的最近条目。`cargo run -p wisp-core --example peek`

fn main() {
    let db = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join("Wisp")
        .join("wisp.db");

    let conn = rusqlite::Connection::open(&db).expect("打开数据库失败");
    let mut stmt = conn
        .prepare(
            "SELECT id, kind, preview, pinned, length(thumb), length(content)
             FROM clips ORDER BY id DESC LIMIT 8",
        )
        .expect("查询失败");

    println!("{:<5} {:<5} {:<6} {:<8} {:<10} preview", "id", "kind", "pinned", "thumb", "content");
    let rows = stmt
        .query_map([], |row| {
            Ok(format!(
                "{:<5} {:<5} {:<6} {:<8} {:<10} {}",
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(3)?,
                format!("{:?}", row.get::<_, Option<i64>>(4)?),
                format!("{:?}", row.get::<_, Option<i64>>(5)?),
                row.get::<_, String>(2)?,
            ))
        })
        .expect("遍历失败");
    for row in rows {
        println!("{}", row.unwrap());
    }
}
