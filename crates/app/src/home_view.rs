//! 主页：uTools 风格的功能入口网格。
//!
//! 唤起即落在主页——搜索框过滤功能，回车或点击进入功能页；
//! 规划中的功能置灰占位，顺带充当内置路线图。

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

/// 主页发出的"打开功能页"请求，由根视图订阅并完成切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenFeature {
    Clipboard,
    Memo,
}

/// 一张功能卡片的静态描述。`target` 为 None 表示规划中：置灰、不可进入。
struct Feature {
    name: &'static str,
    /// 搜索别名：中文名之外的英文命中词
    aliases: &'static [&'static str],
    icon: IconName,
    tint: u32,
    target: Option<OpenFeature>,
}

const FEATURES: &[Feature] = &[
    Feature {
        name: "剪贴板",
        aliases: &["clipboard"],
        icon: IconName::Copy,
        tint: 0x378add,
        target: Some(OpenFeature::Clipboard),
    },
    Feature {
        name: "备忘快贴",
        aliases: &["memo", "note"],
        icon: IconName::BookOpen,
        tint: 0x7f77dd,
        target: Some(OpenFeature::Memo),
    },
    // 图像 / 文件不是独立功能，是剪贴板历史的内部分类（见 ClipFilter），不在此占位
    Feature {
        name: "IP 工具",
        aliases: &["ip"],
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

        v_flex()
            .id(("feature", ix))
            .w(px(104.))
            .py_2()
            .gap_1p5()
            .items_center()
            .rounded_lg()
            .when(enabled && selected, |card| card.bg(cx.theme().accent))
            .when(enabled, |card| {
                card.cursor_pointer()
                    .hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
            })
            .when(!enabled, |card| card.opacity(0.35))
            .child(
                div()
                    .size(px(52.))
                    .rounded_xl()
                    .flex()
                    .items_center()
                    .justify_center()
                    // 图标颜色取自文本样式栈，这里统一染白
                    .text_color(white())
                    .bg(rgb(feature.tint))
                    .child(Icon::new(feature.icon.clone()).w(px(26.)).h(px(26.))),
            )
            .child(div().text_sm().child(feature.name))
            .child(
                div()
                    .h(px(16.))
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(if enabled { "" } else { "规划中" }),
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
            .child(div().px_3().pt_2().pb_1().child(Input::new(&self.search_state)))
            .child(
                div()
                    .px_4()
                    .pb_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("功能"),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .px_3()
                    .gap_2()
                    .flex_wrap()
                    .items_start()
                    .children(
                        features
                            .iter()
                            .enumerate()
                            .map(|(ix, feature)| self.render_card(ix, *feature, cx)),
                    ),
            )
            .child(
                h_flex()
                    .px_3()
                    .pb_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .justify_between()
                    .child(format!("{} 个功能", features.len()))
                    .child("↑↓ 选择 · 回车进入 · Esc 隐藏 · Ctrl+1/2 直达"),
            )
    }
}
