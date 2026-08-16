//! 备忘快贴的存储层：片段与标签的多对多关系。
//!
//! 与剪贴板共用同一个 SQLite 文件（WAL 模式下多连接读写安全），
//! 但表与连接彼此独立，两个功能互不阻塞。

use std::{
    path::Path,
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context as _, Result};
use rusqlite::{Connection, params, params_from_iter};

use crate::{
    clip::make_preview,
    memo::{Memo, MemoDraft, TagFilter, TagSummary},
    store::{escape_like, now_ms},
};

pub(crate) struct MemoStore {
    conn: Mutex<Connection>,
}

impl MemoStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)
            .with_context(|| format!("打开备忘数据库失败: {}", db_path.display()))?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(Duration::from_millis(2000))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memos (
                id         INTEGER PRIMARY KEY,
                content    TEXT    NOT NULL,
                note       TEXT    NOT NULL DEFAULT '',
                preview    TEXT    NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tags (
                id   INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE
            );
            CREATE TABLE IF NOT EXISTS memo_tags (
                memo_id INTEGER NOT NULL REFERENCES memos(id) ON DELETE CASCADE,
                tag_id  INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
                PRIMARY KEY (memo_id, tag_id)
            );
            CREATE INDEX IF NOT EXISTS idx_memos_updated ON memos(updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_memo_tags_tag ON memo_tags(tag_id);",
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 按标签与关键字检索。关键字同时匹配内容与备注。
    pub fn list(&self, filter: &TagFilter, keyword: &str) -> Result<Vec<Memo>> {
        let conn = self.conn.lock().expect("memo store poisoned");
        let keyword = keyword.trim();

        // 标签串在 SQL 内聚合，避免逐条回查造成 N+1
        let mut sql = String::from(
            "SELECT m.id, m.content, m.note, m.preview, m.updated_at,
                    COALESCE((SELECT group_concat(t.name, '\u{1}')
                              FROM memo_tags mt JOIN tags t ON t.id = mt.tag_id
                              WHERE mt.memo_id = m.id), '') AS tags
             FROM memos m WHERE 1 = 1",
        );
        let mut args: Vec<String> = Vec::new();

        match filter {
            TagFilter::All => {}
            TagFilter::Untagged => {
                sql.push_str(" AND m.id NOT IN (SELECT memo_id FROM memo_tags)");
            }
            TagFilter::Named(name) => {
                sql.push_str(
                    " AND m.id IN (SELECT mt.memo_id FROM memo_tags mt
                                   JOIN tags t ON t.id = mt.tag_id WHERE t.name = ?)",
                );
                args.push(name.clone());
            }
        }

        if !keyword.is_empty() {
            sql.push_str(" AND (m.content LIKE ? ESCAPE '\\' OR m.note LIKE ? ESCAPE '\\')");
            let pattern = format!("%{}%", escape_like(keyword));
            args.push(pattern.clone());
            args.push(pattern);
        }
        sql.push_str(" ORDER BY m.updated_at DESC");

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
            let tags: String = row.get(5)?;
            Ok(Memo {
                id: row.get(0)?,
                content: row.get(1)?,
                note: row.get(2)?,
                preview: row.get(3)?,
                updated_at: row.get(4)?,
                tags: tags
                    .split('\u{1}')
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 侧栏数据：全部数量、无标签数量、各标签数量。
    pub fn tag_summaries(&self) -> Result<(i64, i64, Vec<TagSummary>)> {
        let conn = self.conn.lock().expect("memo store poisoned");

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM memos", [], |row| row.get(0))?;
        let untagged: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memos WHERE id NOT IN (SELECT memo_id FROM memo_tags)",
            [],
            |row| row.get(0),
        )?;

        let mut stmt = conn.prepare(
            "SELECT t.name, COUNT(mt.memo_id) FROM tags t
             LEFT JOIN memo_tags mt ON mt.tag_id = t.id
             GROUP BY t.id ORDER BY t.name",
        )?;
        let tags = stmt
            .query_map([], |row| {
                Ok(TagSummary {
                    name: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok((total, untagged, tags))
    }

    /// 新建或更新。标签整体替换，并顺带清理不再被引用的孤立标签。
    pub fn save(&self, draft: &MemoDraft) -> Result<i64> {
        let mut conn = self.conn.lock().expect("memo store poisoned");
        let tx = conn.transaction()?;
        let now = now_ms();
        let preview = make_preview(&draft.content);

        let id = match draft.id {
            Some(id) => {
                tx.execute(
                    "UPDATE memos SET content = ?1, note = ?2, preview = ?3, updated_at = ?4
                     WHERE id = ?5",
                    params![draft.content, draft.note, preview, now, id],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO memos (content, note, preview, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?4)",
                    params![draft.content, draft.note, preview, now],
                )?;
                tx.last_insert_rowid()
            }
        };

        tx.execute("DELETE FROM memo_tags WHERE memo_id = ?1", params![id])?;
        for tag in &draft.tags {
            tx.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", params![tag])?;
            tx.execute(
                "INSERT OR IGNORE INTO memo_tags (memo_id, tag_id)
                 VALUES (?1, (SELECT id FROM tags WHERE name = ?2))",
                params![id, tag],
            )?;
        }
        tx.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM memo_tags)",
            [],
        )?;

        tx.commit()?;
        Ok(id)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let mut conn = self.conn.lock().expect("memo store poisoned");
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM memos WHERE id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM memo_tags)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn content_of(&self, id: i64) -> Result<String> {
        let conn = self.conn.lock().expect("memo store poisoned");
        Ok(conn.query_row(
            "SELECT content FROM memos WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_store() -> MemoStore {
        let store = MemoStore {
            conn: Mutex::new(Connection::open_in_memory().unwrap()),
        };
        {
            let conn = store.conn.lock().unwrap();
            conn.pragma_update(None, "foreign_keys", "ON").unwrap();
            conn.execute_batch(
                "CREATE TABLE memos (
                    id INTEGER PRIMARY KEY, content TEXT NOT NULL, note TEXT NOT NULL DEFAULT '',
                    preview TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
                CREATE TABLE memo_tags (
                    memo_id INTEGER NOT NULL REFERENCES memos(id) ON DELETE CASCADE,
                    tag_id  INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
                    PRIMARY KEY (memo_id, tag_id)
                );",
            )
            .unwrap();
        }
        store
    }

    fn draft(content: &str, tags: &[&str]) -> MemoDraft {
        MemoDraft {
            id: None,
            content: content.into(),
            note: String::new(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    #[test]
    fn filters_by_tag_and_untagged() {
        let store = memory_store();
        store.save(&draft("带标签的", &["工作"])).unwrap();
        store.save(&draft("没标签的", &[])).unwrap();

        let tagged = store.list(&TagFilter::Named("工作".into()), "").unwrap();
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].tags, vec!["工作"]);

        let untagged = store.list(&TagFilter::Untagged, "").unwrap();
        assert_eq!(untagged.len(), 1);
        assert_eq!(untagged[0].content, "没标签的");
    }

    #[test]
    fn keyword_matches_content_and_note() {
        let store = memory_store();
        let mut with_note = draft("普通内容", &[]);
        with_note.note = "cloudflare 密钥".into();
        store.save(&with_note).unwrap();
        store.save(&draft("另一条", &[])).unwrap();

        assert_eq!(store.list(&TagFilter::All, "cloudflare").unwrap().len(), 1);
        assert_eq!(store.list(&TagFilter::All, "普通").unwrap().len(), 1);
    }

    #[test]
    fn orphan_tags_are_cleaned_after_retag() {
        let store = memory_store();
        let id = store.save(&draft("内容", &["旧标签"])).unwrap();

        let mut updated = draft("内容", &["新标签"]);
        updated.id = Some(id);
        store.save(&updated).unwrap();

        let (_, _, tags) = store.tag_summaries().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "新标签");
    }
}
