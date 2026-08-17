//! 现代极客风格的通用 UI 微组件与品牌色板（Raycast / Linear 审美规范）。
//!
//! 语义色一律走 `cx.theme()`，随主题自动切换；品牌色不在主题体系内，由本模块
//! 的色板函数按当前明暗档适配后供各视图取用。

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{ActiveTheme as _, StyledExt as _};

// ==================== 品牌色板 ====================

/// Wisp 品牌紫：logo、选中导轨、批量选择徽章的标识色。
pub(crate) const BRAND: u32 = 0x6C5CE7;

/// 置顶/警示琥珀。
const WARNING: u32 = 0xF59E0B;

/// 亮色档的亮度上限。这些色值原本是照暗底调的，直接铺到白底上对比度不足
/// （琥珀 #F59E0B 在白底仅约 2.1:1），统一压到该阈值以下换回可读性。
const LIGHT_LIGHTNESS_CAP: f32 = 0.42;

/// 把品牌类色值适配到当前主题：暗色档原样保留，亮色档压暗。
pub(crate) fn tint(color: u32, cx: &App) -> Hsla {
    let color: Hsla = rgb(color).into();
    if cx.theme().mode.is_dark() {
        color
    } else {
        Hsla {
            l: color.l.min(LIGHT_LIGHTNESS_CAP),
            ..color
        }
    }
}

/// 当前主题下的品牌紫。
pub(crate) fn brand(cx: &App) -> Hsla {
    tint(BRAND, cx)
}

/// 当前主题下的警示琥珀。
pub(crate) fn warning(cx: &App) -> Hsla {
    tint(WARNING, cx)
}

// ==================== 微组件 ====================

/// 现代实体键帽组件（Kbd Pill）
pub(crate) fn kbd_pill(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_1p5()
        .py_0p5()
        .rounded(px(4.))
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .bg(cx.theme().secondary.opacity(0.7))
        .border_1()
        .border_color(cx.theme().border.opacity(0.5))
        .child(text.into())
}

/// 小号标签胶囊（Tag Pill）
#[allow(dead_code)]
pub(crate) fn tag_pill(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .px_1p5()
        .py_0p5()
        .rounded(px(4.))
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .bg(cx.theme().secondary.opacity(0.6))
        .border_1()
        .border_color(cx.theme().border.opacity(0.4))
        .child(text.into())
}

/// 分类/类型微徽章（Kind Badge）
#[allow(dead_code)]
pub(crate) fn kind_badge(label: &str, active: bool, cx: &App) -> Div {
    div()
        .px_2()
        .py_0p5()
        .rounded(px(5.))
        .text_xs()
        .when(active, |this| {
            this.bg(cx.theme().accent)
                .text_color(cx.theme().accent_foreground)
                .font_medium()
        })
        .when(!active, |this| {
            this.text_color(cx.theme().muted_foreground)
                .bg(cx.theme().secondary.opacity(0.5))
                .border_1()
                .border_color(cx.theme().border.opacity(0.3))
        })
        .child(label.to_string())
}
