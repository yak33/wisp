//! 备忘快贴：标签化的文本片段库，搜索后回车直接粘贴。
//!
//! 两种形态共用一个视图——列表态浏览与检索，编辑态新建与修改，
//! 由 [`MemoView::editor`] 是否存在切换。

use std::{
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, VirtualListScrollHandle,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    scroll::{ScrollableElement as _, ScrollbarAxis},
    tooltip::Tooltip,
    v_flex, v_virtual_list,
};
use wisp_core::{Memo, MemoDraft, MemoService, TagFilter, TagSummary, parse_tags};

use crate::{
    hide_main_window, paste_target,
    ui::{brand, kbd_pill},
};

const ROW_HEIGHT: Pixels = px(56.);
const ROW_WIDTH: Pixels = px(560.);
const SIDEBAR_WIDTH: Pixels = px(140.);
/// 悬停提示延迟：鼠标扫过列表时避免连续弹出
const TOOLTIP_DELAY: Duration = Duration::from_millis(300);
/// 悬停预览最多渲染的字符数，超出部分以尾注交代
const TOOLTIP_PREVIEW_CHARS: usize = 500;

/// 编辑态持有的三个输入框。取消编辑即整体丢弃，无需回滚。
struct MemoEditor {
    id: Option<i64>,
    content: Entity<TextareaState>,
    note: Entity<InputState>,
    tags: Entity<InputState>,
}

pub(crate) struct MemoView {
    service: Arc<MemoService>,
    search_state: Entity<InputState>,
    keyword: String,
    filter: TagFilter,
    total: i64,
    untagged: i64,
    tags: Vec<TagSummary>,
    items: Vec<Memo>,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    selected: usize,
    scroll_handle: VirtualListScrollHandle,
    editor: Option<MemoEditor>,
    _subscriptions: Vec<Subscription>,
}

impl MemoView {
    pub fn new(service: Arc<MemoService>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_state =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索备忘，回车粘贴选中项…"));

        let _subscriptions = vec![cx.subscribe_in(
            &search_state,
            window,
            |this: &mut Self, state, ev, _, cx| match ev {
                InputEvent::Change => {
                    this.keyword = state.read(cx).value().to_string();
                    // 搜索即重选首条，与剪贴板视图一致
                    this.selected = 0;
                    this.reload(cx);
                }
                InputEvent::PressEnter { secondary, .. } => this.deliver_selected(!secondary, cx),
                _ => {}
            },
        )];

        let mut view = Self {
            service,
            search_state,
            keyword: String::new(),
            filter: TagFilter::All,
            total: 0,
            untagged: 0,
            tags: Vec::new(),
            items: Vec::new(),
            item_sizes: Rc::new(Vec::new()),
            selected: 0,
            scroll_handle: VirtualListScrollHandle::new(),
            editor: None,
            _subscriptions,
        };
        view.reload(cx);
        view
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        // 编辑态下焦点属于内容框，不要抢回搜索框
        if self.editor.is_none() {
            self.search_state
                .update(cx, |state, cx| state.focus(window, cx));
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let (total, untagged, tags) = self.service.tag_summaries();
        self.total = total;
        self.untagged = untagged;
        self.tags = tags;

        self.items = self.service.list(&self.filter, &self.keyword);
        self.item_sizes = Rc::new(vec![size(ROW_WIDTH, ROW_HEIGHT); self.items.len()]);
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
        cx.notify();
    }

    fn set_filter(&mut self, filter: TagFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        self.selected = 0;
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        self.reload(cx);
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() as i64 - 1;
        self.selected = (self.selected as i64 + delta).clamp(0, last) as usize;
        self.scroll_handle
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// 交付选中项：`paste` 为真时直接粘贴到唤起前的窗口，否则仅回填剪贴板。
    fn deliver_selected(&mut self, paste: bool, cx: &mut Context<Self>) {
        let Some(memo) = self.items.get(self.selected) else {
            return;
        };
        let id = memo.id;
        let target = paste.then(|| paste_target(cx)).flatten();

        hide_main_window(cx);
        _ = self.service.paste_to(id, target);
    }

    fn open_editor(&mut self, memo: Option<&Memo>, window: &mut Window, cx: &mut Context<Self>) {
        // 从当前筛选标签起草，新建时省去重复输入
        let default_tags = match (memo, &self.filter) {
            (Some(memo), _) => memo.tags.join("，"),
            (None, TagFilter::Named(name)) => name.clone(),
            _ => String::new(),
        };
        let (id, content, note) = match memo {
            Some(memo) => (Some(memo.id), memo.content.clone(), memo.note.clone()),
            None => (None, String::new(), String::new()),
        };

        let content_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(8)
                .placeholder("在此粘贴或输入要保存的文本…")
                .default_value(content)
        });
        let note_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("备注，例如：cloudflare 密钥")
                .default_value(note)
        });
        let tags_state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("标签，逗号分隔")
                .default_value(default_tags)
        });

        content_state.update(cx, |state, cx| state.focus(window, cx));
        self.editor = Some(MemoEditor {
            id,
            content: content_state,
            note: note_state,
            tags: tags_state,
        });
        cx.notify();
    }

    fn edit_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(memo) = self.items.get(self.selected).cloned() {
            self.open_editor(Some(&memo), window, cx);
        }
    }

    fn save_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = &self.editor else {
            return;
        };
        let content = editor.content.read(cx).value().trim().to_string();
        if content.is_empty() {
            return; // 空内容不落库，直接维持编辑态
        }

        let draft = MemoDraft {
            id: editor.id,
            content,
            note: editor.note.read(cx).value().trim().to_string(),
            tags: parse_tags(&editor.tags.read(cx).value()),
        };

        if self.service.save(&draft).is_ok() {
            self.close_editor(window, cx);
        }
    }

    fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = None;
        self.reload(cx);
        self.focus_search(window, cx);
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(memo) = self.items.get(self.selected) {
            let id = memo.id;
            if self.service.delete(id).is_ok() {
                self.reload(cx);
            }
        }
    }

    // ==================== 渲染 ====================

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entries = [
            ("全部", self.total, TagFilter::All),
            ("无标签", self.untagged, TagFilter::Untagged),
        ]
        .into_iter()
        .map(|(name, count, filter)| (name.to_string(), count, filter))
        .chain(
            self.tags
                .iter()
                .map(|tag| (tag.name.clone(), tag.count, TagFilter::Named(tag.name.clone()))),
        );

        v_flex()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .py_1p5()
            .gap_1()
            .border_r_1()
            .border_color(cx.theme().border.opacity(0.35))
            .children(entries.enumerate().map(|(ix, (name, count, filter))| {
                let active = self.filter == filter;
                h_flex()
                    .id(("tag", ix))
                    .mx_1p5()
                    .px_2p5()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .justify_between()
                    .items_center()
                    .cursor_pointer()
                    .when(active, |row| {
                        row.bg(cx.theme().accent)
                            .text_color(cx.theme().accent_foreground)
                            .font_medium()
                    })
                    .when(!active, |row| {
                        row.hover(|style| style.bg(cx.theme().accent.opacity(0.35)))
                    })
                    .child(div().overflow_hidden().whitespace_nowrap().child(name))
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(4.))
                            .text_xs()
                            .when(active, |badge| {
                                badge
                                    .bg(cx.theme().accent_foreground.opacity(0.15))
                                    .text_color(cx.theme().accent_foreground)
                            })
                            .when(!active, |badge| {
                                badge
                                    .bg(cx.theme().secondary.opacity(0.6))
                                    .text_color(cx.theme().muted_foreground)
                            })
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.set_filter(filter.clone(), cx);
                    }))
            }))
    }

    fn render_row(&self, ix: usize, cx: &Context<Self>) -> Div {
        let memo = &self.items[ix];
        let active = ix == self.selected;
        let footnote = row_footnote(memo);

        h_flex()
            .w_full()
            .h(ROW_HEIGHT)
            .px_3()
            .gap_2p5()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.3))
            .when(active, |row| row.bg(cx.theme().accent.opacity(0.85)))
            .when(!active, |row| {
                row.hover(|style| style.bg(cx.theme().accent.opacity(0.3)))
            })
            // 左侧发光 Accent 导轨
            .child(
                div()
                    .w(px(3.))
                    .h(px(24.))
                    .rounded_full()
                    .when(active, |bar| bar.bg(brand(cx)))
                    .when(!active, |bar| bar.opacity(0.)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_0p5()
                    .justify_center()
                    .child(
                        div()
                            .overflow_hidden()
                            .text_sm()
                            .font_medium()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(memo.preview.clone()),
                    )
                    .when(!footnote.is_empty(), |body| {
                        body.child(
                            div()
                                .overflow_hidden()
                                .text_xs()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(cx.theme().muted_foreground)
                                .child(footnote),
                        )
                    }),
            )
    }

    /// 渲染一个列表项：行 + 悬停预览 + 点击交付。
    fn render_item(&self, ix: usize, cx: &Context<Self>) -> Stateful<Div> {
        let memo = &self.items[ix];
        let char_count = memo.content.chars().count() as i64;
        let preview = tooltip_preview(&memo.content, char_count);
        let mut footnote = row_footnote(memo);
        if !footnote.is_empty() {
            footnote.push_str(" · ");
        }
        footnote.push_str(&format!("共 {char_count} 字符"));

        self.render_row(ix, cx)
            .id(ix)
            .tooltip_show_delay(TOOLTIP_DELAY)
            .tooltip(move |window, cx| {
                Tooltip::element({
                    let preview = preview.clone();
                    let footnote = footnote.clone();
                    move |_, cx| memo_tooltip_body(preview.clone(), footnote.clone(), cx)
                })
                .build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = ix;
                this.deliver_selected(true, cx);
            }))
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .child(
                div().relative().size_full().child(
                    v_flex()
                        .id("memo-list")
                        .relative()
                        .size_full()
                        .child(
                            v_virtual_list(
                                cx.entity().clone(),
                                "memo-items",
                                self.item_sizes.clone(),
                                |this, visible_range, _, cx| {
                                    visible_range
                                        .filter_map(|ix| {
                                            if ix >= this.items.len() {
                                                return None;
                                            }
                                            Some(this.render_item(ix, cx))
                                        })
                                        .collect()
                                },
                            )
                            .track_scroll(&self.scroll_handle),
                        )
                        .scrollbar(&self.scroll_handle, ScrollbarAxis::Vertical),
                ),
            )
    }

    fn render_editor(&self, editor: &MemoEditor, cx: &mut Context<Self>) -> impl IntoElement {
        let is_new = editor.id.is_none();

        v_flex()
            .size_full()
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(if is_new { "新建备忘" } else { "编辑备忘" }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(kbd_pill("Ctrl+S 保存", cx))
                            .child(kbd_pill("Esc 取消", cx)),
                    ),
            )
            .child(Textarea::new(&editor.content).h(px(200.)))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&editor.note)))
                    .child(div().flex_1().child(Input::new(&editor.tags))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("memo-save")
                            .primary()
                            .small()
                            .label("保存备忘")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.save_editor(window, cx)
                            })),
                    )
                    .child(
                        Button::new("memo-cancel")
                            .ghost()
                            .small()
                            .label("取消")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.close_editor(window, cx)
                            })),
                    ),
            )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_selection = !self.items.is_empty();

        h_flex()
            .px_3p5()
            .py_2()
            .gap_2()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(cx.theme().border.opacity(0.35))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("memo-new")
                            .primary()
                            .xsmall()
                            .label("新建")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor(None, window, cx)
                            })),
                    )
                    .child(
                        Button::new("memo-edit")
                            .outline()
                            .xsmall()
                            .label("编辑")
                            .disabled(!has_selection)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_selected(window, cx)
                            })),
                    )
                    .child(
                        Button::new("memo-delete")
                            .danger()
                            .xsmall()
                            .label("删除")
                            .disabled(!has_selection)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx))),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(kbd_pill("Ctrl+N", cx))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("新建"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(kbd_pill("Ctrl+E", cx))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("编辑"),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(kbd_pill("↵", cx))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("粘贴"),
                            ),
                    ),
            )
    }
}

/// 行内小字与悬停尾注共用：备注在前、标签在后，空段自动省略。
fn row_footnote(memo: &Memo) -> String {
    match (memo.note.is_empty(), memo.tags.is_empty()) {
        (true, true) => String::new(),
        (false, true) => memo.note.clone(),
        (true, false) => memo.tags.join(" · "),
        (false, false) => format!("{} — {}", memo.note, memo.tags.join(" · ")),
    }
}

/// 悬停预览正文：保留换行截断至 TOOLTIP_PREVIEW_CHARS 字符，超出补省略号。
fn tooltip_preview(content: &str, char_count: i64) -> String {
    let mut preview: String = content.chars().take(TOOLTIP_PREVIEW_CHARS).collect();
    if char_count as usize > TOOLTIP_PREVIEW_CHARS {
        preview.push('…');
    }
    preview
}

/// 悬停提示正文：截断预览 + 备注/标签/总字符数尾注。
///
/// 换行用逐行子元素保留（此版 gpui 的 WhiteSpace 无 pre-wrap），
/// 空行以空格占位维持行高。
fn memo_tooltip_body(preview: String, footnote: String, cx: &App) -> Div {
    v_flex()
        .max_w(px(480.))
        .py_1()
        .gap_1()
        .child(
            v_flex()
                .min_w(px(320.))
                .max_h(px(320.))
                .overflow_hidden()
                .text_sm()
                .line_height(relative(1.5))
                .children(preview.lines().map(|line| {
                    div().child(if line.is_empty() { " ".to_string() } else { line.to_string() })
                })),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(footnote),
        )
}

impl Render for MemoView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let ctrl = ev.keystroke.modifiers.control;
                match ev.keystroke.key.as_str() {
                    // 编辑态的 Esc 是"取消编辑"，消费掉不再外冒；
                    // 列表态不处理 Esc，冒泡到根视图统一处理（回主页）
                    "escape" if this.is_editing() => {
                        this.close_editor(window, cx);
                        cx.stop_propagation();
                    }
                    "s" if ctrl && this.is_editing() => this.save_editor(window, cx),
                    // 列表态才响应导航与增删，编辑态的方向键属于文本框
                    _ if this.is_editing() => {}
                    "up" => this.move_selection(-1, cx),
                    "down" => this.move_selection(1, cx),
                    "n" if ctrl => this.open_editor(None, window, cx),
                    "e" if ctrl => this.edit_selected(window, cx),
                    _ => {}
                }
            }))
            .map(|body| match &self.editor {
                Some(editor) => {
                    // 借出 editor 渲染，避免与 &mut self 冲突
                    let editor = MemoEditor {
                        id: editor.id,
                        content: editor.content.clone(),
                        note: editor.note.clone(),
                        tags: editor.tags.clone(),
                    };
                    body.child(self.render_editor(&editor, cx))
                }
                None => body
                    .child(div().px_3p5().pt_3().pb_2().child(Input::new(&self.search_state)))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_sidebar(cx))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .size_full()
                                    .when(self.items.is_empty(), |body| {
                                        body.child(
                                            div()
                                                .p_6()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("还没有备忘，点「新建」或按 Ctrl+N 添加"),
                                        )
                                    })
                                    .when(!self.items.is_empty(), |body| {
                                        body.child(self.render_list(cx))
                                    }),
                            ),
                    )
                    .child(self.render_toolbar(cx)),
            })
    }
}
