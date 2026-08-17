//! 交付竞争探针：复刻应用运行时拓扑（watcher + worker 存活），
//! 测量图像交付后、剪贴板被自家 worker 占用的时间窗。
//!
//! `cargo run -p wisp-core --example contention_probe`
//!
//! 采样线共 60 个点，每点 10ms（覆盖交付后 600ms）：
//! `.` = 该时刻剪贴板可打开；`X` = 被占用（模拟目标应用 OpenClipboard 失败）。
//! 第二行的 `^` 标记 paste.rs 发送 Ctrl+V 的时刻（+80ms）。

use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use windows::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};

fn main() {
    let db = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join("Wisp")
        .join("wisp.db");

    // 与应用一致：watcher + worker 全程存活
    let (changed_tx, _changed_rx) = crossbeam_channel::unbounded();
    let service = wisp_core::ClipboardService::start(&db, changed_tx).expect("启动剪贴板服务失败");

    let clips = service.query(wisp_core::ClipFilter::Image, "", 1);
    let Some(clip) = clips.first() else {
        println!("库中没有图像条目，先截一张图再跑探针");
        return;
    };
    println!("交付图像 #{}（{}）", clip.id, clip.preview);

    // 采样线程：模拟目标应用在交付后的 600ms 内随时可能 OpenClipboard
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        for _ in 0..60 {
            thread::sleep(Duration::from_millis(10));
            let free = unsafe { OpenClipboard(None).is_ok() };
            if free {
                unsafe {
                    _ = CloseClipboard();
                }
            }
            line.push(if free { '.' } else { 'X' });
        }
        _ = done_tx.send(line);
    });

    let started = Instant::now();
    if let Err(err) = service.copy_to_clipboard(clip.id) {
        println!("copy_to_clipboard 失败: {err:#}");
        return;
    }
    println!("copy_to_clipboard 耗时 {:?}", started.elapsed());

    let line = done_rx.recv().expect("采样线程异常");
    println!("占用时间线: {line}");
    println!("Ctrl+V 时刻:        ^（+80ms，第 8 个点）");

    // 给 worker 留足跑完一轮的时间再退出，避免 Drop 竞争干扰结论
    thread::sleep(Duration::from_millis(800));
    drop(service);
}
