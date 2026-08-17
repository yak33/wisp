//! 应用自有资源：在 gpui-component-assets 之上叠加本地图标。
//!
//! gpui-component 未收录的产品图标统一在这里内嵌为单色 SVG，经 [`WispIcon`]
//! 实现 [`IconNamed`] 接入
//! `Button::icon` 等组件体系。gpui 把 SVG 当单色 alpha 蒙版光栅化后整体着色，
//! 故图标内的 fill/stroke 取值不影响最终颜色，只有覆盖区域参与渲染。

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
            "icons/pin.svg" => Ok(Some(Cow::Borrowed(
                include_bytes!("../assets/icons/pin.svg") as &[u8],
            ))),
            "icons/pin-off.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/pin-off.svg"
            ) as &[u8]))),
            "icons/logo.svg" => Ok(Some(Cow::Borrowed(
                include_bytes!("../assets/icons/logo.svg") as &[u8],
            ))),
            "icons/theme-system.svg" => Ok(Some(Cow::Borrowed(include_bytes!(
                "../assets/icons/theme-system.svg"
            ) as &[u8]))),
            "icons/trash.svg" => Ok(Some(Cow::Borrowed(
                include_bytes!("../assets/icons/trash.svg") as &[u8],
            ))),
            _ => self.base.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut entries = self.base.list(path)?;
        for name in [
            "icons/pin.svg",
            "icons/pin-off.svg",
            "icons/logo.svg",
            "icons/theme-system.svg",
            "icons/trash.svg",
        ] {
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
    /// 轻烟精灵本体（窗口标题旁的品牌图形）
    Logo,
    /// 主题跟随系统（圆环半实心，明暗各半）
    ThemeSystem,
    /// 清空剪贴板历史
    Trash,
}

impl IconNamed for WispIcon {
    fn path(self) -> SharedString {
        match self {
            Self::Pin => "icons/pin.svg".into(),
            Self::PinOff => "icons/pin-off.svg".into(),
            Self::Logo => "icons/logo.svg".into(),
            Self::ThemeSystem => "icons/theme-system.svg".into(),
            Self::Trash => "icons/trash.svg".into(),
        }
    }
}
