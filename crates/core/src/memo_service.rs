//! 备忘快贴服务：对壳层暴露的编排入口。

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result};

use crate::{
    memo::{Memo, MemoDraft, TagFilter, TagSummary},
    memo_store::MemoStore,
    paste,
};

/// 交付回环抑制窗：与 ClipboardService 共享同一套时长语义。
/// 写回剪贴板后，watcher 在该时长内忽略监听信号，防止把自家交付读回入库。
const SUPPRESS_WINDOW_MS: i64 = 500;

pub struct MemoService {
    store: MemoStore,
    /// 与 ClipboardService 共享的抑制窗截止时刻（Unix 毫秒）。
    /// 打开时传入，交付前同步开启，交付失败立即关闭。
    suppress_until: Arc<AtomicI64>,
}

impl MemoService {
    pub fn open(db_path: &Path, suppress_until: Arc<AtomicI64>) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("创建数据目录失败: {}", dir.display()))?;
        }
        Ok(Self {
            store: MemoStore::open(db_path)?,
            suppress_until,
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

    /// 仅复制到剪贴板（不粘贴）。在独立线程执行，UI 零阻塞。
    pub fn copy_to_clipboard(self: &Arc<Self>, id: i64) {
        let service = Arc::clone(self);
        thread::spawn(move || {
            if let Err(err) = service.deliver(id, None) {
                eprintln!("复制备忘失败: {err:#}");
            }
        });
    }

    /// 写回剪贴板并粘贴到 `target`；`target` 为空时退化为仅复制。
    /// 在独立线程执行，UI 零阻塞。
    pub fn paste_to(self: &Arc<Self>, id: i64, target: Option<isize>) {
        let service = Arc::clone(self);
        thread::spawn(move || {
            if let Err(err) = service.deliver(id, target) {
                eprintln!("交付备忘失败: {err:#}");
            }
        });
    }

    /// 交付核心：读内容 → 开抑制窗 → 写回剪贴板 → 粘贴。
    /// 写回前开抑制窗，写回失败立即关窗，不误伤用户正常复制。
    fn deliver(&self, id: i64, target: Option<isize>) -> Result<()> {
        let content = self.store.content_of(id)?;

        // 写回前开抑制窗：WM_CLIPBOARDUPDATE 在 CloseClipboard 后即触发，
        // 必须先落窗再写，窗口期才能完整覆盖 watcher→worker 的唤醒延迟
        self.suppress_until
            .store(now_ms() + SUPPRESS_WINDOW_MS, Ordering::Relaxed);

        let written = retry_write(5, || {
            arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(content.clone()))
        });

        match written {
            Ok(()) => {
                if let Some(target) = target {
                    paste::paste_into(target);
                }
                Ok(())
            }
            Err(err) => {
                // 写入未发生，无回环可抑——立即关窗，不误伤用户的正常复制
                self.suppress_until.store(0, Ordering::Relaxed);
                Err(err)
            }
        }
    }
}

/// 剪贴板独占竞争的写入重试：每次重试完整重建 open→write→close 序列。
fn retry_write(
    attempts: usize,
    mut write: impl FnMut() -> std::result::Result<(), arboard::Error>,
) -> Result<()> {
    let mut last: Option<arboard::Error> = None;
    for attempt in 0..attempts {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(30));
        }
        match write() {
            Ok(()) => return Ok(()),
            Err(err) => last = Some(err),
        }
    }
    let detail = last
        .map(|err| err.to_string())
        .unwrap_or_else(|| "未知错误".into());
    Err(anyhow::anyhow!(
        "写入系统剪贴板失败（已重试 {attempts} 次）: {detail}"
    ))
}

/// 与 service.rs 保持一致的毫秒时间戳，避免循环依赖。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
