//! 备忘快贴服务：对壳层暴露的编排入口。

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::{
    memo::{Memo, MemoDraft, TagFilter, TagSummary},
    memo_store::MemoStore,
    paste,
};

pub struct MemoService {
    store: MemoStore,
}

impl MemoService {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建数据目录失败: {}", dir.display()))?;
        }
        Ok(Self {
            store: MemoStore::open(db_path)?,
        })
    }

    pub fn list(&self, filter: &TagFilter, keyword: &str) -> Vec<Memo> {
        self.store.list(filter, keyword).unwrap_or_default()
    }

    /// 侧栏数据：(全部数量, 无标签数量, 各标签)
    pub fn tag_summaries(&self) -> (i64, i64, Vec<TagSummary>) {
        self.store
            .tag_summaries()
            .unwrap_or_else(|_| (0, 0, Vec::new()))
    }

    pub fn save(&self, draft: &MemoDraft) -> Result<i64> {
        self.store.save(draft)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.store.delete(id)
    }

    /// 写回剪贴板并粘贴到 `target`；`target` 为空时退化为仅复制。
    pub fn paste_to(&self, id: i64, target: Option<isize>) -> Result<()> {
        let content = self.store.content_of(id)?;
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(content))
            .context("写入系统剪贴板失败")?;

        if let Some(target) = target {
            paste::paste_into(target);
        }
        Ok(())
    }
}
