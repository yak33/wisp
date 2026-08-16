//! 剪贴板服务：对壳层暴露的唯一编排入口。
//!
//! 线程拓扑：
//! - 监听线程（watcher）：Win32 消息循环，变更时发一个信号；
//! - 工作线程（worker）：收信号 → 读剪贴板 → 入库 → 通知壳层刷新；
//! - 壳层线程：只做查询与写回剪贴板，全部走 [`ClipboardService`] 方法。

use std::{io::Cursor, path::Path, sync::Arc, thread, time::Duration};

use anyhow::{Context as _, Result};
use crossbeam_channel::Sender;

use crate::{
    clip::{Clip, ClipFilter, ClipKind, fingerprint},
    paste,
    store::ClipStore,
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
        let images_dir = db_path
            .parent()
            .map(|dir| dir.join("images"))
            .unwrap_or_else(|| Path::new("images").to_path_buf());

        let (clipboard_tx, clipboard_rx) = crossbeam_channel::unbounded::<()>();
        let watcher = watcher::start(clipboard_tx)?;

        let worker_store = Arc::clone(&store);
        thread::Builder::new()
            .name("wisp-clipboard-worker".into())
            .spawn(move || {
                for () in clipboard_rx.iter() {
                    // 三格式探测：文本 → 图像 → 文件列表
                    // （写入方可能还占着剪贴板，各带短退避重试）
                    let inserted = if let Some(text) = read_clipboard_text_with_retry() {
                        if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
                            false
                        } else {
                            worker_store.insert_text(&text).is_ok()
                        }
                    } else if let Some(image) = read_clipboard_image() {
                        match persist_image(&images_dir, &image) {
                            Some((hash, path, preview, thumb)) => {
                                worker_store.insert_image(hash, &path, &preview, &thumb).is_ok()
                            }
                            None => false,
                        }
                    } else if let Some(files) = read_clipboard_files() {
                        insert_files(&worker_store, &files)
                    } else {
                        false
                    };

                    if inserted {
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
    /// 文本走 arboard；图像走裸 Win32 双格式写入——arboard 3.6.1 的
    /// Windows 图像路径在本机持续失败（SetClipboardData 1418）。
    pub fn copy_to_clipboard(&self, id: i64) -> Result<()> {
        let (kind, content) = self.store.kind_and_content(id)?;

        match kind {
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
        }
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
