//! 现代极客风格的通用 UI 微组件（Raycast / Linear 审美规范）。

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{ActiveTheme as _, StyledExt as _};

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
