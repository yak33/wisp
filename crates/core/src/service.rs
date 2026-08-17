//! 剪贴板服务：对壳层暴露的唯一编排入口。
//!
//! 线程拓扑：
//! - 监听线程（watcher）：Win32 消息循环，变更时发一个信号；
//! - 工作线程（worker）：收信号 → 读剪贴板 → 入库 → 通知壳层刷新；
//! - 壳层线程：只做查询与写回剪贴板，全部走 [`ClipboardService`] 方法。

use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;

use crate::{
    clip::{Clip, ClipFilter, ClipKind, fingerprint},
    paste,
    store::{ClipStore, now_ms},
    watcher,
};

/// 超过该体量的文本不进历史（防止异常复制撑爆列表与磁盘）
const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
/// 图像像素熔断：4K 截图（约 830 万像素）以内全收，再大不入库
const MAX_IMAGE_PIXELS: u64 = 4096 * 4096;
/// 缩略图最长边（列表行 40px，2x 屏仍清晰）
const THUMB_MAX_EDGE: u32 = 128;
/// 单次文件列表的最大文件数熔断
const MAX_FILE_COUNT: usize = 100;
/// 交付回环抑制窗：交付写回剪贴板后，worker 在该时长内忽略监听信号。
/// 须覆盖 watcher→worker 的唤醒延迟，且远大于 Ctrl+V 的发出时刻（+80ms）。
const SUPPRESS_WINDOW_MS: i64 = 500;

pub struct ClipboardService {
    store: Arc<ClipStore>,
    images_dir: PathBuf,
    /// 串行化“图片落盘 + 入库”与批量清理，避免新截图落到已被清理的路径。
    mutation_guard: Arc<Mutex<()>>,
    changed_tx: Sender<()>,
    /// 交付回环抑制截止时刻（Unix 毫秒）。worker 唤醒时早于该时刻则跳过。
    suppress_until: Arc<AtomicI64>,
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
        let images_dir = db_path
            .parent()
            .map(|dir| dir.join("images"))
            .unwrap_or_else(|| Path::new("images").to_path_buf());
        // 孤儿图像随启动一并扫除：prune 与手动删除只删行，落盘文件在此回收
        sweep_orphan_images(&store, &images_dir);

        let (clipboard_tx, clipboard_rx) = crossbeam_channel::unbounded::<()>();
        let watcher = watcher::start(clipboard_tx)?;

        let suppress_until = Arc::new(AtomicI64::new(0));
        let mutation_guard = Arc::new(Mutex::new(()));
        let worker_store = Arc::clone(&store);
        let worker_suppress = Arc::clone(&suppress_until);
        let worker_mutation_guard = Arc::clone(&mutation_guard);
        let worker_images_dir = images_dir.clone();
        let worker_changed_tx = changed_tx.clone();
        thread::Builder::new()
            .name("wisp-clipboard-worker".into())
            .spawn(move || {
                for () in clipboard_rx.iter() {
                    // 交付回环抑制：交付（复制/粘贴）是自家写回，其信号不入库。
                    // 若此时读剪贴板做回收，会与目标应用在 Ctrl+V 时刻竞争
                    // OpenClipboard——实测图像交付后 ~+300ms 出现 30ms 级占用窗，
                    // 恰覆盖 paste 线程发出 Ctrl+V 的位置，导致粘贴静默落空。
                    if worker_suppress.load(Ordering::Relaxed) > now_ms() {
                        continue;
                    }
                    // 三格式探测：文本 → 图像 → 文件列表
                    // （写入方可能还占着剪贴板，各带短退避重试）
                    let _mutation = worker_mutation_guard
                        .lock()
                        .expect("clipboard mutation guard poisoned");
                    let inserted = if let Some(text) = read_clipboard_text_with_retry() {
                        if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
                            false
                        } else {
                            worker_store.insert_text(&text).is_ok()
                        }
                    } else if let Some(image) = read_clipboard_image() {
                        match persist_image(&worker_images_dir, &image) {
                            Some((hash, path, preview, thumb)) => worker_store
                                .insert_image(hash, &path, &preview, &thumb)
                                .is_ok(),
                            None => false,
                        }
                    } else if let Some(files) = read_clipboard_files() {
                        insert_files(&worker_store, &files)
                    } else {
                        false
                    };

                    if inserted {
                        worker_store.prune();
                        _ = worker_changed_tx.try_send(());
                    }
                }
            })
            .context("启动剪贴板工作线程失败")?;

        Ok(Self {
            store,
            images_dir,
            mutation_guard,
            changed_tx,
            suppress_until,
            _watcher: watcher,
        })
    }

    /// 按分类与关键字检索。空关键字返回该分类下最近记录；置顶项恒在最前。
    pub fn query(&self, filter: ClipFilter, keyword: &str, limit: usize) -> Vec<Clip> {
        self.store.query(filter, keyword, limit).unwrap_or_default()
    }

    /// 将指定条目写回系统剪贴板并回顶（写回前开启回环抑制窗，
    /// worker 不再把自家交付读回入库——回顶由 touch 确定性完成）。
    /// 文本走 arboard；图像走裸 Win32 双格式写入——arboard 3.6.1 的
    /// Windows 图像路径在本机持续失败（SetClipboardData 1418）。
    pub fn copy_to_clipboard(&self, id: i64) -> Result<()> {
        let (kind, content) = self.store.kind_and_content(id)?;

        // 写回前开抑制窗：WM_CLIPBOARDUPDATE 在 CloseClipboard 后即触发，
        // 必须先落窗再写，窗口期才能完整覆盖 watcher→worker 的唤醒延迟
        self.suppress_until
            .store(now_ms() + SUPPRESS_WINDOW_MS, Ordering::Relaxed);

        let written = match kind {
            ClipKind::Image => {
                let png = std::fs::read(&content)
                    .with_context(|| format!("读取图像文件失败: {content}"))?;
                crate::clip_write::write_image_png(&png)
            }
            ClipKind::Files => {
                let paths: Vec<std::path::PathBuf> =
                    content.lines().map(std::path::PathBuf::from).collect();
                crate::clip_write::write_files(&paths)
            }
            _ => retry_write(5, || {
                arboard::Clipboard::new()
                    .and_then(|mut clipboard| clipboard.set_text(content.clone()))
            }),
        };

        match written {
            Ok(()) => {
                // 回顶与列表刷新都不再依赖回收回环；抑制窗内 worker 静默
                _ = self.store.touch(id);
                _ = self.changed_tx.try_send(());
                Ok(())
            }
            Err(err) => {
                // 写入未发生，无回环可抑——立即关窗，不误伤用户的正常复制
                self.suppress_until.store(0, Ordering::Relaxed);
                Err(err)
            }
        }
    }

    /// 写回剪贴板并直接粘贴到 `target` 窗口；`target` 为空时退化为仅复制。
    ///
    /// 图像分支含解码与 DIB 转换（实测百毫秒级），故整个交付在独立线程
    /// 执行——UI 线程零阻塞，失败只能进日志（交付是即发即弃动作）。
    ///
    /// 调用方需已隐藏自身窗口，否则焦点还原会与窗口显隐竞争。
    pub fn paste_to(self: &Arc<Self>, id: i64, target: Option<isize>) {
        let service = Arc::clone(self);
        thread::spawn(move || {
            if let Err(err) = service.copy_to_clipboard(id) {
                eprintln!("交付失败: {err:#}");
                return;
            }
            if let Some(target) = target {
                paste::paste_into(target);
            }
        });
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

    /// 全部未收藏记录数。用于在破坏性操作前明确告知影响范围。
    pub fn unpinned_count(&self) -> usize {
        self.store.unpinned_count().unwrap_or_default()
    }

    /// 清空全部未收藏记录，并立即回收这些记录对应的受管图像文件。
    pub fn clear_unpinned(&self) -> Result<usize> {
        let _mutation = self
            .mutation_guard
            .lock()
            .expect("clipboard mutation guard poisoned");
        let cleared = self.store.clear_unpinned()?;

        for image_path in &cleared.image_paths {
            remove_managed_image(&self.images_dir, image_path);
        }

        Ok(cleared.count)
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

fn read_clipboard_image() -> Option<arboard::ImageData<'static>> {
    for attempt in 0..3 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(30));
        }
        if let Ok(image) = arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
            return Some(image);
        }
    }
    None
}

fn read_clipboard_files() -> Option<Vec<std::path::PathBuf>> {
    for attempt in 0..3 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(30));
        }
        if let Ok(files) = arboard::Clipboard::new().and_then(|mut c| c.get().file_list()) {
            return Some(files);
        }
    }
    None
}

/// 文件列表入库：路径按行拼接为 content，摘要取首文件名。
fn insert_files(store: &ClipStore, paths: &[std::path::PathBuf]) -> bool {
    if paths.is_empty() || paths.len() > MAX_FILE_COUNT {
        return false;
    }
    let content = paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    let preview = match paths {
        [only] => only.file_name().map_or_else(|| content.clone(), |n| n.to_string_lossy().into_owned()),
        _ => {
            let first = paths[0]
                .file_name()
                .map_or_else(|| paths[0].to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
            format!("{first} 等 {} 个文件", paths.len())
        }
    };
    store
        .insert_files(fingerprint(content.as_bytes()), &content, &preview)
        .is_ok()
}

/// 图像落盘与缩略图：原图按内容哈希命名存入数据目录（天然去重），
/// 缩略图（最长边 128px 的 PNG）随库行走。返回
/// (hash, 原图路径, "宽×高 · 体积" 摘要, 缩略图字节)。
fn persist_image(
    dir: &Path,
    image: &arboard::ImageData<'_>,
) -> Option<(i64, String, String, Vec<u8>)> {
    use image::imageops::FilterType;
    use image::ImageFormat;

    let (width, height) = (image.width as u32, image.height as u32);
    let bytes = image.bytes.as_ref();
    if width == 0
        || height == 0
        || bytes.len() != width as usize * height as usize * 4
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return None;
    }

    let hash = fingerprint(bytes);
    let buffer = image::RgbaImage::from_raw(width, height, bytes.to_vec())?;

    // 原图 PNG 编码（无损，截图/图形类内容压缩率高）
    let mut png = Vec::new();
    buffer
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{hash:016x}.png"));
    if !path.exists() {
        std::fs::write(&path, &png).ok()?;
    }

    // 缩略图
    let scale = f64::from(THUMB_MAX_EDGE) / f64::from(width.max(height));
    let thumb_image = if scale < 1.0 {
        image::imageops::resize(
            &buffer,
            (f64::from(width) * scale).round() as u32,
            (f64::from(height) * scale).round() as u32,
            FilterType::Triangle,
        )
    } else {
        buffer
    };
    let mut thumb = Vec::new();
    thumb_image
        .write_to(&mut Cursor::new(&mut thumb), ImageFormat::Png)
        .ok()?;

    let preview = format!("{width}×{height} · {}", human_size(png.len()));
    Some((hash, path.to_string_lossy().into_owned(), preview, thumb))
}

/// 孤儿图像清理：行已删（过期/封顶/手动删除）而落盘文件残留。
/// 与 prune 同哲学——失败静默，清理是护栏不是关键路径。
fn sweep_orphan_images(store: &ClipStore, images_dir: &Path) {
    let keep: std::collections::HashSet<String> = store
        .image_file_names()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let Ok(entries) = std::fs::read_dir(images_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".png") && !keep.contains(&name) {
            _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 只回收 Wisp 图像目录中的 PNG，数据库内容异常时也不越界删除用户文件。
fn remove_managed_image(images_dir: &Path, image_path: &Path) {
    if !is_managed_image_path(images_dir, image_path) {
        return;
    }

    if let Err(err) = std::fs::remove_file(image_path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("清理剪贴板图像失败（{}）: {err}", image_path.display());
    }
}

fn is_managed_image_path(images_dir: &Path, image_path: &Path) -> bool {
    image_path.parent() == Some(images_dir)
        && image_path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * KB;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if (bytes as f64) < MB {
        format!("{:.1} KB", bytes as f64 / KB)
    } else {
        format!("{:.1} MB", bytes as f64 / MB)
    }
}

/// 剪贴板独占竞争的写入重试：每次重试完整重建 open→write→close 序列。
fn retry_write<T>(
    attempts: usize,
    mut write: impl FnMut() -> std::result::Result<T, arboard::Error>,
) -> Result<T> {
    let mut last: Option<arboard::Error> = None;
    for attempt in 0..attempts {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(30));
        }
        match write() {
            Ok(value) => return Ok(value),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_image_cleanup_never_crosses_the_images_directory() {
        let images_dir = Path::new(r"C:\Wisp\data\images");

        assert!(is_managed_image_path(
            images_dir,
            Path::new(r"C:\Wisp\data\images\00ff.png")
        ));
        assert!(!is_managed_image_path(
            images_dir,
            Path::new(r"C:\Wisp\data\outside.png")
        ));
        assert!(!is_managed_image_path(
            images_dir,
            Path::new(r"C:\Wisp\data\images\notes.txt")
        ));
    }
}
