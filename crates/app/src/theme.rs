//! 主题偏好：跟随系统 / 亮色 / 暗色 三态，持久化于 [`Config`]。
//!
//! gpui-component 的 [`Theme`] 只有 Light/Dark 二态，"跟随系统"是本层语义——
//! 落盘存三态，激活时把 System 解析为当前系统外观。系统深浅色切换的实时跟随
//! 由 `wisp_view` 订阅 `observe_window_appearance` 后重新激活本偏好完成。

use gpui::{App, Window};
use gpui_component::{Icon, IconName, Theme, ThemeMode};

use crate::{assets::WispIcon, config};

/// 用户选择的主题偏好。点击标题栏按钮按 System → Light → Dark 循环。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ThemePreference {
    /// 跟随系统外观，系统切换时实时响应
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    /// 设置页选项的展示顺序。
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// 配置是唯一事实来源：标题栏与设置页都从这里读，天然同步。
    pub fn current(cx: &App) -> Self {
        config::get("theme", cx)
            .and_then(|key| Self::from_key(&key))
            .unwrap_or_default()
    }

    /// 选定偏好：立即生效并落盘。标题栏循环与设置页点选共用此入口。
    pub fn select(self, window: Option<&mut Window>, cx: &mut App) {
        self.activate(window, cx);
        config::set("theme", self.key(), cx);
    }

    /// 循环到下一档，供单按钮三态切换使用。
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::System => Icon::new(WispIcon::ThemeSystem),
            Self::Light => Icon::new(IconName::Sun),
            Self::Dark => Icon::new(IconName::Moon),
        }
    }

    /// 档位名，同时用作标题栏按钮提示与设置页选项标签。
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "浅色模式",
            Self::Dark => "深色模式",
        }
    }

    /// 激活本偏好。`window` 为 None 时不触发重绘，供首帧之前的初始化使用。
    ///
    /// 副作用：改写全局 [`Theme`] 与 `gpui_base::Theme`（滚动条/resize 手柄投影）。
    pub fn activate(self, window: Option<&mut Window>, cx: &mut App) {
        match self {
            // 读窗口实况外观；无窗口时回落到 cx.window_appearance()
            Self::System => Theme::sync_system_appearance(window, cx),
            Self::Light => Theme::change(ThemeMode::Light, window, cx),
            Self::Dark => Theme::change(ThemeMode::Dark, window, cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘键与解析必须严格互逆；未知值（旧配置、手工误改）回落跟随系统。
    #[test]
    fn keys_round_trip_and_unknown_falls_back() {
        let cases = [
            (ThemePreference::System, "system"),
            (ThemePreference::Light, "light"),
            (ThemePreference::Dark, "dark"),
        ];

        for (preference, key) in cases {
            assert_eq!(preference.key(), key);
            assert_eq!(ThemePreference::from_key(key), Some(preference));
        }

        for unknown in ["", "auto", "Dark", "系统"] {
            assert_eq!(ThemePreference::from_key(unknown), None);
        }
        assert_eq!(ThemePreference::default(), ThemePreference::System);
    }

    /// 三态循环必须闭环，且三档全覆盖——单按钮切换的前提。
    #[test]
    fn cycling_visits_every_mode_and_returns() {
        let start = ThemePreference::System;
        let visited = [start, start.next(), start.next().next()];

        assert_eq!(
            visited,
            [
                ThemePreference::System,
                ThemePreference::Light,
                ThemePreference::Dark
            ]
        );
        assert_eq!(start.next().next().next(), start);
    }
}
