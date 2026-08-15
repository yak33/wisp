//! SQLite 存储层：入库去重、置顶、检索。
//!
//! 5 万量级下 `LIKE` 全表扫描仍是毫秒级，第一版刻意不引入
//! FTS5 + 中文分词的复杂度；量级或延迟出现拐点时再升级。

use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, params};

use crate::clip::{Clip, ClipKind, fingerprint, make_preview};

pub(crate) struct ClipStore {
    conn: Mutex<Connection>,
}

impl ClipStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("打开剪贴板数据库失败: {}", db_path.display()))?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS clips (
                id         INTEGER PRIMARY KEY,
                kind       INTEGER NOT NULL DEFAULT 0,
                content    TEXT    NOT NULL,
                preview    TEXT    NOT NULL,
                hash       INTEGER NOT NULL,
                pinned     INTEGER NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_clips_hash ON clips(hash);
            CREATE INDEX IF NOT EXISTS idx_clips_order ON clips(pinned DESC, created_at DESC);",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 文本入库。重复内容不新增记录，仅刷新时间戳使其回到列表顶部
    /// （包括用户从 Wisp 复制回剪贴板触发的自我回环，这正是期望行为）。
    pub fn insert_text(&self, content: &str) -> Result<()> {
        let conn = self.conn.lock().expect("clip store poisoned");
        let now = now_ms();
        let hash = fingerprint(content);

        let touched = conn.execute(
            "UPDATE clips SET created_at = ?1 WHERE hash = ?2 AND content = ?3",
            params![now, hash, content],
        )?;

        if touched == 0 {
            conn.execute(
                "INSERT INTO clips (kind, content, preview, hash, pinned, created_at)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![
                    ClipKind::Text as i64,
                    content,
                    make_preview(content),
                    hash,
                    now
                ],
            )?;
        }
        Ok(())
    }

    /// 检索：空关键字返回最近记录，否则按内容模糊匹配；置顶项恒排最前。
    pub fn query(&self, keyword: &str, limit: usize) -> Result<Vec<Clip>> {
        let conn = self.conn.lock().expect("clip store poisoned");
        let keyword = keyword.trim();

        let (sql, pattern);
        if keyword.is_empty() {
            sql = "SELECT id, kind, content, preview, pinned, created_at FROM clips
                   ORDER BY pinned DESC, created_at DESC LIMIT ?1";
            pattern = String::new();
        } else {
            sql = "SELECT id, kind, content, preview, pinned, created_at FROM clips
                   WHERE content LIKE ?2 ESCAPE '\\'
                   ORDER BY pinned DESC, created_at DESC LIMIT ?1";
            pattern = format!("%{}%", escape_like(keyword));
        }

        let mut stmt = conn.prepare_cached(sql)?;
        let map_row = |row: &rusqlite::Row<'_>| {
            let content: String = row.get(2)?;
            Ok(Clip {
                id: row.get(0)?,
                kind: ClipKind::from_i64(row.get(1)?),
                char_count: content.chars().count() as i64,
                content,
                preview: row.get(3)?,
                pinned: row.get::<_, i64>(4)? != 0,
                created_at: row.get(5)?,
            })
        };

        let rows = if keyword.is_empty() {
            stmt.query_map(params![limit as i64], map_row)?
        } else {
            stmt.query_map(params![limit as i64, pattern], map_row)?
        };

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn content_of(&self, id: i64) -> Result<String> {
        let conn = self.conn.lock().expect("clip store poisoned");
        Ok(conn.query_row(
            "SELECT content FROM clips WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?)
    }

    pub fn toggle_pin(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().expect("clip store poisoned");
        conn.execute("UPDATE clips SET pinned = 1 - pinned WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock().expect("clip store poisoned");
        conn.execute("DELETE FROM clips WHERE id = ?1", params![id])?;
        Ok(())
    }
}

fn escape_like(keyword: &str) -> String {
    keyword
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_store() -> ClipStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE clips (
                id INTEGER PRIMARY KEY, kind INTEGER NOT NULL DEFAULT 0,
                content TEXT NOT NULL, preview TEXT NOT NULL, hash INTEGER NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        ClipStore {
            conn: Mutex::new(conn),
        }
    }

    #[test]
    fn duplicate_content_moves_to_top_instead_of_duplicating() {
        let store = memory_store();
        store.insert_text("第一条").unwrap();
        store.insert_text("第二条").unwrap();
        store.insert_text("第一条").unwrap();

        let clips = store.query("", 10).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].content, "第一条");
    }

    #[test]
    fn like_wildcards_are_treated_literally() {
        let store = memory_store();
        store.insert_text("进度 100%").unwrap();
        store.insert_text("进度未知").unwrap();

        let clips = store.query("100%", 10).unwrap();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].content, "进度 100%");
    }

    #[test]
    fn pinned_clips_stay_on_top() {
        let store = memory_store();
        store.insert_text("普通").unwrap();
        store.insert_text("置顶").unwrap();
        store.insert_text("最新").unwrap();

        let pinned_id = store.query("置顶", 1).unwrap()[0].id;
        store.toggle_pin(pinned_id).unwrap();

        let clips = store.query("", 10).unwrap();
        assert_eq!(clips[0].content, "置顶");
    }
}
