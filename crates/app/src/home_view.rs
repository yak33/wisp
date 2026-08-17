//! 主页：Raycast / Linear 风格的功能入口与 Command Bar。
//!
//! 唤起即落在主页——搜索框过滤功能，回车或点击进入功能页；
//! 规划中的功能置灰占位，顺带充当内置路线图。

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex,
    input::{InputEvent, InputState},
    v_flex,
};

use crate::ui::{kbd_pill, search_input, tint};

/// 主页发出的"打开功能页"请求，由根视图订阅并完成切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenFeature {
    Clipboard,
    Memo,
}

/// 一张功能卡片的静态描述。`target` 为 None 表示规划中：置灰、不可进入。
struct Feature {
    name: &'static str,
    desc: &'static str,
    /// 搜索别名：中文名之外的英文或拼音命中词
    aliases: &'static [&'static str],
    shortcut: &'static str,
    icon: IconName,
    tint: u32,
    target: Option<OpenFeature>,
}

const FEATURES: &[Feature] = &[
    Feature {
        name: "剪贴板",
        desc: "历史记录 · 文本与截图即时存取",
        aliases: &["clipboard", "clip", "jt"],
        shortcut: "Ctrl+1",
        icon: IconName::Copy,
        tint: 0x378add,
        target: Some(OpenFeature::Clipboard),
    },
    Feature {
        name: "备忘快贴",
        desc: "片段管理 · 常用代码与标签化文本",
        aliases: &["memo", "note", "bw"],
        shortcut: "Ctrl+2",
        icon: IconName::BookOpen,
        tint: 0x7f77dd,
        target: Some(OpenFeature::Memo),
    },
    // 图像 / 文件不是独立功能，是剪贴板历史的内部分类（见 ClipFilter），不在此占位
    Feature {
        name: "IP 工具",
        desc: "网络工具 · 三段 IP 与延迟面板",
        aliases: &["ip", "wl"],
        shortcut: "规划中",
        icon: IconName::Globe,
        tint: 0x1d9e75,
        target: None,
    },
];

pub(crate) struct HomeView {
    search_state: Entity<InputState>,
    keyword: String,
    selected: usize,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<OpenFeature> for HomeView {}

impl HomeView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_state = cx.new(|cx| InputState::new(window, cx).placeholder("搜索功能，回车进入…"));

        let _subscriptions = vec![cx.subscribe_in(
            &search_state,
            window,
            |this: &mut Self, state, ev, _, cx| match ev {
                InputEvent::Change => {
                    this.keyword = state.read(cx).value().to_string();
                    this.selected = 0;
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.open_selected(cx),
                _ => {}
            },
        )];

        Self {
            search_state,
            keyword: String::new(),
            selected: 0,
            _subscriptions,
        }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_state
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// 回到主页时选择项归位；关键字保留，方便连续过滤后直接进入。
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.selected = 0;
        cx.notify();
    }

    /// 中文名或英文别名命中即保留；空关键字返回全部。
    fn filtered(&self) -> Vec<&'static Feature> {
        let keyword = self.keyword.trim().to_lowercase();
        FEATURES
            .iter()
            .filter(|feature| {
                keyword.is_empty()
                    || feature.name.contains(keyword.as_str())
                    || feature
                        .aliases
                        .iter()
                        .any(|alias| alias.contains(keyword.as_str()))
            })
            .collect()
    }

    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let features = self.filtered();
        let Some(feature) = features.get(self.selected).or(features.first()) else {
            return;
        };
        if let Some(target) = feature.target {
            cx.emit(target);
        }
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        let len = self.filtered().len();
        if len == 0 {
            return;
        }
        let last = len as i64 - 1;
        self.selected = (self.selected as i64 + delta).clamp(0, last) as usize;
        cx.notify();
    }

    // ==================== 渲染 ====================

    // 返回具体类型 Stateful<Div>：RPIT 会捕获 &self/cx 生命周期，导致卡片无法逃出 map 闭包
    fn render_card(&self, ix: usize, feature: &'static Feature, cx: &Context<Self>) -> Stateful<Div> {
        let enabled = feature.target.is_some();
        let selected = ix == self.selected;
        // 功能卡面积大，选中层级应比列表行更轻，避免形成沉重的整块灰面。
        let selected_background = cx
            .theme()
            .foreground
            .opacity(if cx.theme().mode.is_dark() { 0.08 } else { 0.06 });
        let selected_edge = cx
            .theme()
            .foreground
            .opacity(if cx.theme().mode.is_dark() { 0.28 } else { 0.14 });

        v_flex()
            .id(("feature", ix))
            .w(px(216.))
            .p_3()
            .gap_2()
            .rounded_xl()
            .bg(cx.theme().secondary.opacity(0.45))
            .border_1()
            .when(enabled && selected, |card| {
                card.border_color(selected_edge).bg(selected_background)
            })
            .when(!selected, |card| {
                card.border_color(cx.theme().border.opacity(0.35))
            })
            .when(enabled, |card| {
                card.cursor_pointer().hover(|style| {
                    style
                        .bg(cx.theme().accent.opacity(0.35))
                        .border_color(cx.theme().border.opacity(0.6))
                })
            })
            .when(!enabled, |card| card.opacity(0.4))
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .size(px(36.))
                            .rounded_lg()
                            .bg(tint(feature.tint, cx).opacity(0.15))
                            .text_color(tint(feature.tint, cx))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(feature.icon.clone()).w(px(20.)).h(px(20.))),
                    )
                    .child(kbd_pill(feature.shortcut, cx)),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().font_semibold().text_sm().child(feature.name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(feature.desc),
                    ),
            )
            .when_some(feature.target, |card, target| {
                card.on_click(cx.listener(move |_, _, _, cx| cx.emit(target)))
            })
    }
}

impl Render for HomeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let features = self.filtered();

        v_flex()
            .size_full()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                match ev.keystroke.key.as_str() {
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .px_3p5()
                    .pt_3()
                    .pb_2()
                    .child(search_input(&self.search_state, cx)),
            )
            .child(
                div()
                    .px_4()
                    .pb_2()
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child("功能推荐"),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .px_3p5()
                    .gap_2p5()
                    .flex_wrap()
                    .items_start()
                    .children(
                        features
                            .iter()
                            .enumerate()
                            .map(|(ix, feature)| self.render_card(ix, feature, cx)),
                    ),
            )
            .child(
                h_flex()
                    .px_3p5()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .justify_between()
                    .items_center()
                    .border_t_1()
                    .border_color(cx.theme().border.opacity(0.35))
                    .child(format!("{} 个功能", features.len()))
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(kbd_pill("↑↓", cx))
                                    .child("选择"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(kbd_pill("↵", cx))
                                    .child("进入"),
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child(kbd_pill("Esc", cx))
                                    .child("隐藏"),
                            ),
                    ),
            )
    }
}
