//! 应用根视图：标签页切换与窗口级行为（聚焦、失焦隐藏）。
//!
//! 焦点与显隐这类窗口级关注点集中在此处，子视图只负责各自的内容，
//! 避免多个子视图同时抢焦点。

use std::sync::Arc;

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, StyledExt as _,
    checkbox::Checkbox,
    h_flex,
    tab::{Tab, TabBar},
    v_flex,
};
use wisp_core::{ClipboardService, MemoService};

use crate::{WakeHotkey, clipboard_view::ClipboardView, hide_main_window, memo_view::MemoView};

const TAB_CLIPBOARD: usize = 0;
const TAB_MEMO: usize = 1;

pub(crate) struct WispView {
    tab_ix: usize,
    clipboard: Entity<ClipboardView>,
    memos: Entity<MemoView>,
    auto_hide: bool,
    _subscriptions: Vec<Subscription>,
}

impl WispView {
    pub fn new(
        clipboard_service: Arc<ClipboardService>,
        memo_service: Arc<MemoService>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let clipboard = cx.new(|cx| ClipboardView::new(clipboard_service, window, cx));
        let memos = cx.new(|cx| MemoView::new(memo_service, window, cx));

        let _subscriptions = vec![cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active() {
                this.focus_active_tab(window, cx);
                this.reload_active_tab(cx);
            } else if this.auto_hide {
                hide_main_window(cx);
            }
        })];

        Self {
            tab_ix: TAB_CLIPBOARD,
            clipboard,
            memos,
            auto_hide: true,
            _subscriptions,
        }
    }

    /// 剪贴板有新内容时刷新——仅在该标签页可见时才有意义。
    pub fn reload_clips(&self, cx: &mut Context<Self>) {
        if self.tab_ix == TAB_CLIPBOARD {
            self.clipboard.update(cx, |view, cx| view.reload(cx));
        }
    }

    fn reload_active_tab(&self, cx: &mut Context<Self>) {
        match self.tab_ix {
            TAB_MEMO => self.memos.update(cx, |view, cx| view.reload(cx)),
            _ => self.clipboard.update(cx, |view, cx| view.reload(cx)),
        }
    }

    fn focus_active_tab(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab_ix {
            TAB_MEMO => self
                .memos
                .update(cx, |view, cx| view.focus_search(window, cx)),
            _ => self
                .clipboard
                .update(cx, |view, cx| view.focus_search(window, cx)),
        }
    }

    fn select_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_ix = ix;
        self.reload_active_tab(cx);
        self.focus_active_tab(window, cx);
        cx.notify();
    }
}

impl Render for WispView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hotkey = cx.try_global::<WakeHotkey>().map_or("快捷键", |k| k.0);

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.modifiers.control {
                    match ev.keystroke.key.as_str() {
                        "1" => this.select_tab(TAB_CLIPBOARD, window, cx),
                        "2" => this.select_tab(TAB_MEMO, window, cx),
                        _ => {}
                    }
                }
            }))
            .child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(div().font_semibold().child("Wisp"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{hotkey} 显隐")),
                            ),
                    )
                    .child(
                        Checkbox::new("auto-hide")
                            .checked(self.auto_hide)
                            .label("失焦自动隐藏")
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.auto_hide = *checked;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().px_2().pt_2().child(
                    TabBar::new("wisp-tabs")
                        .selected_index(self.tab_ix)
                        .on_click(cx.listener(|this, ix: &usize, window, cx| {
                            this.select_tab(*ix, window, cx);
                        }))
                        .child(Tab::new().label("剪贴板"))
                        .child(Tab::new().label("备忘快贴")),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .map(|body| match self.tab_ix {
                        TAB_MEMO => body.child(self.memos.clone()),
                        _ => body.child(self.clipboard.clone()),
                    }),
            )
    }
}
