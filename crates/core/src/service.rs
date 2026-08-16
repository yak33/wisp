//! 剪贴板服务：对壳层暴露的唯一编排入口。
//!
//! 线程拓扑：
//! - 监听线程（watcher）：Win32 消息循环，变更时发一个信号；
//! - 工作线程（worker）：收信号 → 读剪贴板 → 入库 → 通知壳层刷新；
//! - 壳层线程：只做查询与写回剪贴板，全部走 [`ClipboardService`] 方法。

use std::{path::Path, sync::Arc, thread, time::Duration};

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;

use crate::{
    clip::{Clip, ClipFilter},
    paste,
    store::ClipStore,
    watcher,
};

/// 超过该体量的文本不进历史（防止异常复制撑爆列表与磁盘）
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;

pub struct ClipboardService {
    store: Arc<ClipStore>,
    _watcher: watcher::ClipboardWatcher,
}

impl ClipboardService {
    /// 启动监听与入库流水线。每当有新内容入库，向 `changed_tx` 发一个信号，
    /// 壳层在自己的事件泵里消费并刷新列表。
    pub fn start(db_path: &Path, changed_tx: Sender<()>) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建数据目录失败: {}", dir.display()))?;
        }
        let store = Arc::new(ClipStore::open(db_path)?);
        // 启动即清理一次：长期常驻的进程不能只靠入库触发
        store.prune();

        let (clipboard_tx, clipboard_rx) = crossbeam_channel::unbounded::<()>();
        let watcher = watcher::start(clipboard_tx)?;

        let worker_store = Arc::clone(&store);
        thread::Builder::new()
            .name("wisp-clipboard-worker".into())
            .spawn(move || {
                for () in clipboard_rx.iter() {
                    // 写入方可能还占着剪贴板，短退避重试
                    let Some(text) = read_clipboard_text_with_retry() else {
                        continue;
                    };
                    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
                        continue;
                    }
                    if worker_store.insert_text(&text).is_ok() {
                        worker_store.prune();
                        _ = changed_tx.try_send(());
                    }
                }
            })
            .context("启动剪贴板工作线程失败")?;

        Ok(Self {
            store,
            _watcher: watcher,
        })
    }

    /// 按分类与关键字检索。空关键字返回该分类下最近记录；置顶项恒在最前。
    pub fn query(&self, filter: ClipFilter, keyword: &str, limit: usize) -> Vec<Clip> {
        self.store.query(filter, keyword, limit).unwrap_or_default()
    }

    /// 将指定条目写回系统剪贴板（触发监听回环，该条自动置顶）。
    pub fn copy_to_clipboard(&self, id: i64) -> Result<()> {
        let content = self.store.content_of(id)?;
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(content))
            .context("写入系统剪贴板失败")
    }

    /// 写回剪贴板并直接粘贴到 `target` 窗口；`target` 为空时退化为仅复制。
    ///
    /// 调用方需已隐藏自身窗口，否则焦点还原会与窗口显隐竞争。
    pub fn paste_to(&self, id: i64, target: Option<isize>) -> Result<()> {
        self.copy_to_clipboard(id)?;
        if let Some(target) = target {
            paste::paste_into(target);
        }
        Ok(())
    }

    pub fn toggle_pin(&self, id: i64) -> Result<()> {
        self.store.toggle_pin(id)
    }

    /// 收藏组内拖动排序：`moved_id` 移到 `before_id` 之前，`None` 为组尾。
    pub fn reorder_pinned(&self, moved_id: i64, before_id: Option<i64>) -> Result<()> {
        self.store.reorder_pinned(moved_id, before_id)
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        self.store.delete(id)
    }
}

fn read_clipboard_text_with_retry() -> Option<String> {
    for attempt in 0..3 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(30));
        }
        if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            return Some(text);
        }
    }
    None
}
