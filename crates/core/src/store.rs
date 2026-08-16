//! SQLite 存储层：入库去重、置顶、检索。
//!
//! 5 万量级下 `LIKE` 全表扫描仍是毫秒级，第一版刻意不引入
//! FTS5 + 中文分词的复杂度；量级或延迟出现拐点时再升级。

use std::{
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, params, params_from_iter};

use crate::clip::{Clip, ClipFilter, ClipKind, fingerprint, make_preview};

/// 过期条目的保留时长。置顶（收藏）豁免——用户明确留下的内容不做静默清理。
const RETENTION_MS: i64 = 90 * 24 * 3600 * 1000;
/// 未置顶条目的总量上限，超出时最旧的先走。M3 图像入库前的护栏。
const MAX_ROWS: usize = 2000;

pub(crate) struct ClipStore {
    conn: Mutex<Connection>,
}

impl ClipStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("打开剪贴板数据库失败: {}", db_path.display()))?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(Duration::from_millis(2000))?;
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

    /// 检索：分类与关键字可叠加；空关键字返回该分类下最近记录，置顶项恒排最前。
    pub fn query(&self, filter: ClipFilter, keyword: &str, limit: usize) -> Result<Vec<Clip>> {
        let conn = self.conn.lock().expect("clip store poisoned");
        let keyword = keyword.trim();

        let mut sql = String::from(
            "SELECT id, kind, content, preview, pinned, created_at FROM clips WHERE 1 = 1",
        );
        let mut args: Vec<String> = Vec::new();

        let kind = match filter {
            ClipFilter::All => None,
            ClipFilter::Text => Some(ClipKind::Text),
            ClipFilter::Image => Some(ClipKind::Image),
            ClipFilter::Files => Some(ClipKind::Files),
            ClipFilter::Pinned => {
                sql.push_str(" AND pinned = 1");
                None
            }
        };
        if let Some(kind) = kind {
            sql.push_str(" AND kind = ?");
            args.push((kind as i64).to_string());
        }
        if !keyword.is_empty() {
            sql.push_str(" AND content LIKE ? ESCAPE '\\'");
            args.push(format!("%{}%", escape_like(keyword)));
        }
        sql.push_str(" ORDER BY pinned DESC, created_at DESC LIMIT ?");
        args.push(limit.to_string());

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
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
        })?;

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

    /// 例行清理：过期未置顶条目先走，再把未置顶总量压回上限。
    /// 启动与每次入库后各跑一次；失败静默——清理只是护栏不是关键路径。
    pub(crate) fn prune(&self) {
        let _ = self.prune_before(now_ms() - RETENTION_MS);
        let _ = self.enforce_cap(MAX_ROWS);
    }

    /// 删除早于 `cutoff_ms` 的未置顶条目，返回删除行数。
    fn prune_before(&self, cutoff_ms: i64) -> Result<usize> {
        let conn = self.conn.lock().expect("clip store poisoned");
        Ok(conn.execute(
            "DELETE FROM clips WHERE pinned = 0 AND created_at < ?1",
            params![cutoff_ms],
        )?)
    }

    /// 未置顶总量压回 `keep` 条（按时间新旧保留），置顶豁免，返回删除行数。
    fn enforce_cap(&self, keep: usize) -> Result<usize> {
        let conn = self.conn.lock().expect("clip store poisoned");
        Ok(conn.execute(
            "DELETE FROM clips WHERE pinned = 0 AND id NOT IN (
                 SELECT id FROM clips WHERE pinned = 0
                 ORDER BY created_at DESC, id DESC LIMIT ?1
             )",
            params![keep as i64],
        )?)
    }
}

/// LIKE 模式中的通配符按字面匹配（`%`/`_`/`\`），配合 `ESCAPE '\\'` 使用。
pub(crate) fn escape_like(keyword: &str) -> String {
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

        let clips = store.query(ClipFilter::All, "", 10).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].content, "第一条");
    }

    #[test]
    fn like_wildcards_are_treated_literally() {
        let store = memory_store();
        store.insert_text("进度 100%").unwrap();
        store.insert_text("进度未知").unwrap();

        let clips = store.query(ClipFilter::All, "100%", 10).unwrap();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].content, "进度 100%");
    }

    #[test]
    fn pinned_clips_stay_on_top() {
        let store = memory_store();
        store.insert_text("普通").unwrap();
        store.insert_text("置顶").unwrap();
        store.insert_text("最新").unwrap();

        let pinned_id = store.query(ClipFilter::All, "置顶", 1).unwrap()[0].id;
        store.toggle_pin(pinned_id).unwrap();

        let clips = store.query(ClipFilter::All, "", 10).unwrap();
        assert_eq!(clips[0].content, "置顶");
    }

    #[test]
    fn filter_by_pinned_and_kind() {
        let store = memory_store();
        store.insert_text("普通文本").unwrap();
        store.insert_text("收藏文本").unwrap();

        let pinned_id = store.query(ClipFilter::All, "收藏", 1).unwrap()[0].id;
        store.toggle_pin(pinned_id).unwrap();

        // 收藏分类只含置顶项，且可与关键字叠加
        let pinned = store.query(ClipFilter::Pinned, "", 10).unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].content, "收藏文本");

        let hit = store.query(ClipFilter::Pinned, "收藏", 10).unwrap();
        assert_eq!(hit.len(), 1);
        assert!(store.query(ClipFilter::Pinned, "普通", 10).unwrap().is_empty());

        // 文本分类命中全部文本条目；图像分类暂为空（M3 落地后自动出现）
        assert_eq!(store.query(ClipFilter::Text, "", 10).unwrap().len(), 2);
        assert!(store.query(ClipFilter::Image, "", 10).unwrap().is_empty());
    }

    #[test]
    fn prune_removes_stale_unpinned_but_keeps_pinned() {
        let store = memory_store();
        store.insert_text("过期的普通").unwrap();
        store.insert_text("过期的置顶").unwrap();
        let pinned_id = store.query(ClipFilter::All, "置顶", 1).unwrap()[0].id;
        store.toggle_pin(pinned_id).unwrap();

        // 以未来时刻为界：所有现存条目都算过期
        let removed = store.prune_before(now_ms() + 1000).unwrap();
        assert_eq!(removed, 1);

        let clips = store.query(ClipFilter::All, "", 10).unwrap();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].content, "过期的置顶");
    }

    #[test]
    fn enforce_cap_keeps_newest_unpinned_and_all_pinned() {
        let store = memory_store();
        store.insert_text("第一条").unwrap();
        store.insert_text("第二条").unwrap();
        store.insert_text("第三条").unwrap();
        let pinned_id = store.query(ClipFilter::All, "第一条", 1).unwrap()[0].id;
        store.toggle_pin(pinned_id).unwrap();
        store.insert_text("第四条").unwrap();

        let removed = store.enforce_cap(2).unwrap();
        assert_eq!(removed, 1);

        let clips = store.query(ClipFilter::All, "", 10).unwrap();
        let mut kept: Vec<&str> = clips.iter().map(|clip| clip.content.as_str()).collect();
        kept.sort_unstable();
        // 置顶的第一条豁免；未置顶保留最新的两条（同毫秒按 id 兜底，最旧的是第二条）
        assert_eq!(kept, vec!["第一条", "第三条", "第四条"]);
    }
}
