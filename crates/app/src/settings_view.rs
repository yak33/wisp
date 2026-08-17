//! 设置页：唤起快捷键 / 常规开关 / 外观 / 关于。
//!
//! 所有状态都现读现显——快捷键来自 [`WakeHotkey`] 全局、开关来自注册表与
//! 配置、主题来自 [`ThemePreference::current`]，本视图只保留录制态这一份
//! 自有状态。录制期间会临时注销当前全局键（否则按到旧键就把窗口藏了），
//! 因此**每条退出路径都必须恢复注册**：成功换绑（新键已注册）、Esc 取消、
//! 窗口失焦、切走页面。
//!
//! 安全设计：窗口是 PopUp 形态（不进任务栏/Alt+Tab），托盘一旦隐藏，
//! 快捷键就是唯一入口——页内因此常驻当前快捷键展示与「退出 Wisp」按钮，
//! 录制期间禁用托盘开关。

use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    switch::Switch,
    v_flex,
};

use crate::{
    WakeHotkey, autostart_enabled, config, hotkey, rebind_wake_hotkey, refresh_tray,
    resume_wake_hotkey, set_autostart, set_tray_visible, suspend_wake_hotkey,
    theme::ThemePreference,
    ui::{brand, kbd_pill},
};

const REPO_URL: &str = "https://github.com/yak33/wisp";

pub(crate) struct SettingsView {
    focus_handle: FocusHandle,
    /// 录制新快捷键中（此期间全局唤起键已被临时注销）
    recording: bool,
    /// 最近一次录制/换绑失败的原因，成功或取消后清空
    error: Option<&'static str>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _subscriptions = vec![
            // 失焦兜底：录制中窗口一旦隐藏（点到别处触发自动隐藏），
            // 必须恢复全局键，否则应用再也无法唤起
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if !window.is_window_active() {
                    this.cancel_recording(cx);
                }
            }),
        ];

        Self {
            focus_handle: cx.focus_handle(),
            recording: false,
            error: None,
            _subscriptions,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    /// 录制若在进行则取消并恢复旧键；平时调用是无害空操作。
    /// 切走页面时由根视图调用兜底。
    pub fn cancel_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording {
            self.recording = false;
            self.error = None;
            resume_wake_hotkey(cx);
            cx.notify();
        }
    }

    fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.recording {
            self.recording = true;
            self.error = None;
            // 键事件沿焦点路径分发，焦点若停在「修改」按钮上录制就收不到键
            window.focus(&self.focus_handle, cx);
            suspend_wake_hotkey(cx);
            cx.notify();
        }
    }

    /// 录制态按键处理。合法组合直接换绑；被拒时给出原因（等主键的
    /// 中间态除外）；注册失败（被其他程序占用）时旧键已由回滚恢复。
    fn capture_keystroke(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        if keystroke.key == "escape" {
            self.cancel_recording(cx);
            return;
        }

        match hotkey::from_keystroke(keystroke) {
            Ok(label) => {
                self.recording = false;
                self.error = rebind_wake_hotkey(&label, cx).err();
                cx.notify();
            }
            Err(hotkey::Rejection::ModifierOnly) => {}
            Err(rejection) => {
                self.error = Some(rejection.message());
                cx.notify();
            }
        }
    }

    /// 按默认候选链降级重绑，全部失败才报错（与启动时的探测顺序一致）。
    fn restore_default_hotkey(&mut self, cx: &mut Context<Self>) {
        let mut last_error = None;
        for label in hotkey::DEFAULT_CANDIDATES {
            match rebind_wake_hotkey(label, cx) {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        self.error = last_error;
        cx.notify();
    }

    // ==================== 渲染 ====================

    /// 小节骨架：分组标题 + 内容行。
    fn section(title: &'static str, cx: &App) -> Div {
        v_flex().gap_1p5().child(
            div()
                .text_xs()
                .font_medium()
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
    }

    /// 设置行骨架：左侧名称与说明，右侧控件。
    fn row(name: &'static str, desc: &'static str, cx: &App) -> Div {
        h_flex()
            .px_3()
            .py_2p5()
            .items_center()
            .justify_between()
            .rounded_lg()
            .bg(cx.theme().secondary.opacity(0.45))
            .border_1()
            .border_color(cx.theme().border.opacity(0.35))
            .child(
                v_flex().gap_0p5().child(div().text_sm().child(name)).child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(desc),
                ),
            )
    }

    fn render_hotkey_section(&self, cx: &Context<Self>) -> Div {
        let label = cx
            .try_global::<WakeHotkey>()
            .map_or_else(|| "未注册".to_string(), |k| k.label.clone());

        Self::section("唤起快捷键", cx)
            .child(
                Self::row("全局唤起", "在任意程序中按下即可显示 / 隐藏 Wisp", cx).child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .when(self.recording, |controls| {
                            controls
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .text_xs()
                                        .bg(brand(cx).opacity(0.15))
                                        .text_color(brand(cx))
                                        .child("按下新组合，Esc 取消"),
                                )
                                .child(
                                    Button::new("cancel-record")
                                        .ghost()
                                        .xsmall()
                                        .label("取消")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cancel_recording(cx)),
                                        ),
                                )
                        })
                        .when(!self.recording, |controls| {
                            controls
                                .child(kbd_pill(label, cx))
                                .child(
                                    Button::new("record-hotkey")
                                        .outline()
                                        .xsmall()
                                        .label("修改")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.start_recording(window, cx)
                                        })),
                                )
                                .child(
                                    Button::new("reset-hotkey")
                                        .ghost()
                                        .xsmall()
                                        .label("恢复默认")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.restore_default_hotkey(cx)
                                        })),
                                )
                        }),
                ),
            )
            .when_some(self.error, |section, message| {
                section.child(
                    div()
                        .px_3()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(message),
                )
            })
    }

    fn render_general_section(&self, cx: &Context<Self>) -> Div {
        let tray_hidden = config::get("hide_tray", cx).as_deref() == Some("1");

        Self::section("常规", cx)
            .child(
                Self::row("隐藏托盘图标", "隐藏后仅能通过快捷键唤起、在本页退出", cx).child(
                    Switch::new("hide-tray")
                        .checked(tray_hidden)
                        .disabled(self.recording)
                        .on_click(cx.listener(|_, hidden: &bool, _, cx| {
                            set_tray_visible(!hidden, cx);
                            config::set("hide_tray", if *hidden { "1" } else { "0" }, cx);
                            cx.notify();
                        })),
                ),
            )
            .child(
                Self::row("开机自启", "登录 Windows 后自动在后台运行", cx).child(
                    Switch::new("autostart")
                        .checked(autostart_enabled())
                        .on_click(cx.listener(|_, enabled: &bool, _, cx| {
                            set_autostart(*enabled);
                            refresh_tray(cx); // 托盘菜单同名勾选项跟着对齐
                            cx.notify();
                        })),
                ),
            )
    }

    fn render_appearance_section(&self, cx: &Context<Self>) -> Div {
        let current = ThemePreference::current(cx);

        Self::section("外观", cx).child(
            Self::row("主题", "与标题栏按钮等效", cx).child(h_flex().gap_1().children(
                ThemePreference::ALL.map(|preference| {
                    let active = preference == current;
                    h_flex()
                        .id(preference.key())
                        .px_2p5()
                        .py_1()
                        .gap_1p5()
                        .items_center()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .when(active, |chip| {
                            chip.bg(cx.theme().accent)
                                .text_color(cx.theme().accent_foreground)
                                .font_medium()
                        })
                        .when(!active, |chip| {
                            chip.text_color(cx.theme().muted_foreground)
                                .hover(|style| style.bg(cx.theme().accent.opacity(0.35)))
                        })
                        .child(preference.icon().small())
                        .child(preference.name())
                        .on_click(cx.listener(move |_, _, window, cx| {
                            preference.select(Some(window), cx);
                            cx.notify();
                        }))
                }),
            )),
        )
    }

    fn render_about_section(&self, cx: &Context<Self>) -> Div {
        Self::section("关于", cx).child(
            Self::row(
                "Wisp",
                concat!("v", env!("CARGO_PKG_VERSION"), " · 常驻托盘的效率工具"),
                cx,
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open-repo")
                            .outline()
                            .xsmall()
                            .icon(Icon::new(IconName::Github))
                            .label("GitHub 仓库")
                            .on_click(|_, _, cx| cx.open_url(REPO_URL)),
                    )
                    .child(
                        Button::new("quit-wisp")
                            .danger()
                            .xsmall()
                            .label("退出 Wisp")
                            .on_click(|_, _, cx| cx.quit()),
                    ),
            ),
        )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings-scroll")
            .size_full()
            .track_focus(&self.focus_handle)
            .overflow_y_scrollbar()
            // 录制态独占键盘：捕获组合并截断冒泡，避免触发根视图的
            // Esc 返回 / Ctrl+1/2 切页
            .when(self.recording, |page| {
                page.on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    this.capture_keystroke(&ev.keystroke, cx);
                    cx.stop_propagation();
                }))
            })
            .child(
                v_flex()
                    .w_full()
                    .px_3p5()
                    .pt_3()
                    .pb_4()
                    .gap_4()
                    .child(self.render_hotkey_section(cx))
                    .child(self.render_general_section(cx))
                    .child(self.render_appearance_section(cx))
                    .child(self.render_about_section(cx)),
            )
    }
}
