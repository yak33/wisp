//! 现代极客风格的通用 UI 微组件与标识色板（Raycast / Linear 审美规范）。
//!
//! 语义色一律走 `cx.theme()`，随主题自动切换；功能卡的彩色 tint 不在主题体系内，
//! 由本模块的色板函数按当前明暗档适配后供各视图取用。

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, StyledExt as _,
    input::{Input, InputState},
};

// ==================== 标识色板 ====================

/// 置顶/警示琥珀。
const WARNING: u32 = 0xF59E0B;

/// 亮色档的亮度上限。这些色值原本是照暗底调的，直接铺到白底上对比度不足
/// （琥珀 #F59E0B 在白底仅约 2.1:1），统一压到该阈值以下换回可读性。
const LIGHT_LIGHTNESS_CAP: f32 = 0.42;

/// 把彩色 tint 适配到当前主题：暗色档原样保留，亮色档压暗。
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

/// 标识色：logo、选中边框、选中导轨、计数徽章等强调位统一取主题前景色
/// （亮档深灰 / 暗档浅灰）。去固定品牌紫是有意的克制决策——强调位与正文
/// 同色系，视觉层级靠位置与深浅而非彩色。
pub(crate) fn brand(cx: &App) -> Hsla {
    cx.theme().foreground
}

/// 当前主题下的选中背景。用前景色低透明度叠加，保证浅色和深色都能看出层次，
/// 同时避免直接使用 accent 在浅色档接近白色、选中态反而变得不明显。
pub(crate) fn selection_background(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(if cx.theme().mode.is_dark() {
        0.12
    } else {
        0.10
    })
}

/// 多选但非键盘当前行的弱选中背景。
pub(crate) fn selection_background_subtle(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(if cx.theme().mode.is_dark() {
        0.08
    } else {
        0.055
    })
}

/// 选中边缘提示，替代高对比度的整圈品牌色边框。
pub(crate) fn selection_edge(cx: &App) -> Hsla {
    cx.theme().foreground.opacity(if cx.theme().mode.is_dark() {
        0.42
    } else {
        0.24
    })
}

/// 顶部搜索框的轻量外观。
///
/// 搜索框仍保持逻辑焦点以支持唤起即输入，但不显示组件默认的双层焦点环；
/// 静止时融入页面，鼠标经过才轻微显形。
pub(crate) fn search_input(state: &Entity<InputState>, cx: &App) -> Div {
    let idle_border = cx.theme().border.opacity(if cx.theme().mode.is_dark() {
        0.16
    } else {
        0.14
    });
    let hover_border = cx.theme().border.opacity(if cx.theme().mode.is_dark() {
        0.42
    } else {
        0.34
    });

    div()
        .rounded_lg()
        .border_1()
        .border_color(idle_border)
        .bg(cx.theme().background)
        .hover(|style| {
            style
                .bg(cx.theme().secondary.opacity(0.16))
                .border_color(hover_border)
        })
        .child(Input::new(state).appearance(false))
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
