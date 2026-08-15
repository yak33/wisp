//! Wisp 核心层：剪贴板监听、存储与检索。
//!
//! 本 crate 与 UI 框架零耦合——壳层（GPUI/任何前端）只依赖
//! [`ClipboardService`] 与 [`Clip`] 两个入口类型。

mod clip;
mod service;
mod store;
mod watcher;

pub use clip::{Clip, ClipKind};
pub use service::ClipboardService;
