//! 应用自有资源：在 gpui-component-assets 之上叠加本地图标。
//!
//! gpui-component 未收录 pin 图标，这里以 lucide 同款格式内嵌两个 SVG，
//! 经 [`WispIcon`] 实现 [`IconNamed`] 接入 `Button::icon` 等组件体系。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};
use gpui_component::IconNamed;

/// 叠加资源：先查自有图标，落空再回落到 gpui-component-assets。
pub(crate) struct WispAssets {
    base: gpui_component_assets::Assets,
}

impl WispAssets {
    pub(crate) fn new() -> Self {
        Self {
            base: gpui_component_assets::Assets,
        }
    }
}

impl AssetSource for WispAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match path {
            "icons/pin.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/pin.svg"
            ) as &[u8]))),
            "icons/pin-off.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/pin-off.svg"
            ) as &[u8]))),
            _ => self.base.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries = self.base.list(path)?;
        for name in ["icons/pin.svg", "icons/pin-off.svg"] {
            if name.starts_with(path) {
                entries.push(name.into());
            }
        }
        Ok(entries)
    }
}

/// Wisp 自有图标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WispIcon {
    /// 钉住（失焦不隐藏）
    Pin,
    /// 未钉住（失焦自动隐藏）
    PinOff,
}

impl IconNamed for WispIcon {
    fn path(self) -> SharedString {
        match self {
            Self::Pin => "icons/pin.svg".into(),
            Self::PinOff => "icons/pin-off.svg".into(),
        }
    }
}
