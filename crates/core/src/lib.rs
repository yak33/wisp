//! Wisp 核心层：剪贴板监听、备忘存储与粘贴链路。
//!
//! 本 crate 与 UI 框架零耦合——壳层（GPUI/任何前端）只依赖
//! [`ClipboardService`]、[`MemoService`] 与其数据类型。

mod clip;
mod memo;
mod memo_service;
mod memo_store;
mod paste;
mod service;
mod store;
mod watcher;

pub use clip::{Clip, ClipKind};
pub use memo::{Memo, MemoDraft, TagFilter, TagSummary, parse_tags};
pub use memo_service::MemoService;
pub use paste::capture_foreground;
pub use service::ClipboardService;
