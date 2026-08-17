//! 主题偏好：跟随系统 / 亮色 / 暗色 三态，持久化于 [`Config`]。
//!
//! gpui-component 的 [`Theme`] 只有 Light/Dark 二态，"跟随系统"是本层语义——
//! 落盘存三态，激活时把 System 解析为当前系统外观。系统深浅色切换的实时跟随
//! 由 `wisp_view` 订阅 `observe_window_appearance` 后重新激活本偏好完成。

use std::sync::OnceLock;

use gpui::{App, Window, rgb};
use gpui_component::{Icon, IconName, Theme, ThemeMode};
use windows::{
    Win32::{
        Foundation::HMODULE,
        System::LibraryLoader::{GetProcAddress, LoadLibraryW},
    },
    core::{PCSTR, w},
};
use windows_version::OsVersion;

use crate::{assets::WispIcon, config};

/// 深色模式主文字：避开默认主题接近纯白的高亮度，降低纯黑背景上的眩光。
const DARK_FOREGROUND: u32 = 0xCACCCA;
/// 深色模式辅助文字：与正文保持明确层级，同时确保小字号仍清晰可读。
const DARK_MUTED_FOREGROUND: u32 = 0x8F8F8F;

const WINDOWS_10_1809_BUILD: u32 = 17_763;
const WINDOWS_10_1903_BUILD: u32 = 18_362;
const UXTHEME_REFRESH_IMMERSIVE_COLOR_POLICY_STATE: u16 = 104;
const UXTHEME_SET_PREFERRED_APP_MODE: u16 = 135;
const UXTHEME_FLUSH_MENU_THEMES: u16 = 136;

/// Windows 10 1903+ 的 PreferredAppMode。使用整数边界避免把未公开 ABI
/// 建模为 Rust enum 后引入无效判别值风险。
const APP_MODE_ALLOW_DARK: i32 = 1;
const APP_MODE_FORCE_DARK: i32 = 2;
const APP_MODE_FORCE_LIGHT: i32 = 3;

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
        soften_dark_foreground(cx);
        sync_native_menu_theme(self);
    }

    fn native_app_mode(self) -> i32 {
        match self {
            Self::System => APP_MODE_ALLOW_DARK,
            Self::Light => APP_MODE_FORCE_LIGHT,
            Self::Dark => APP_MODE_FORCE_DARK,
        }
    }
}

/// 让 Win32 `TrackPopupMenu` 与 Wisp 主题同步。
///
/// `tray-icon/muda` 暴露的 `MenuTheme` 只影响窗口菜单栏，托盘右键菜单仍由
/// uxtheme 按进程偏好绘制。这里在主题激活边界一次性同步并清空菜单主题缓存；
/// API 在旧系统上不可用时静默降级为系统默认外观。
fn sync_native_menu_theme(preference: ThemePreference) {
    let version = OsVersion::current();
    if version.major < 10 || version.build < WINDOWS_10_1809_BUILD {
        return;
    }

    let Some(module) = uxtheme_module() else {
        return;
    };

    unsafe {
        if version.build >= WINDOWS_10_1903_BUILD {
            type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
            if let Some(entry) = uxtheme_proc(module, UXTHEME_SET_PREFERRED_APP_MODE) {
                let set_preferred_app_mode: SetPreferredAppMode = std::mem::transmute(entry);
                _ = set_preferred_app_mode(preference.native_app_mode());
            }
        } else {
            // Windows 10 1809 的 ordinal 135 仍是 bool 形态，只能允许/禁止暗色，
            // 无法在浅色系统上强制暗色。
            type AllowDarkModeForApp = unsafe extern "system" fn(bool) -> bool;
            if let Some(entry) = uxtheme_proc(module, UXTHEME_SET_PREFERRED_APP_MODE) {
                let allow_dark_mode_for_app: AllowDarkModeForApp = std::mem::transmute(entry);
                _ = allow_dark_mode_for_app(preference != ThemePreference::Light);
            }
        }

        type RefreshImmersiveColorPolicyState = unsafe extern "system" fn();
        if let Some(entry) = uxtheme_proc(module, UXTHEME_REFRESH_IMMERSIVE_COLOR_POLICY_STATE) {
            let refresh: RefreshImmersiveColorPolicyState = std::mem::transmute(entry);
            refresh();
        }

        type FlushMenuThemes = unsafe extern "system" fn();
        if let Some(entry) = uxtheme_proc(module, UXTHEME_FLUSH_MENU_THEMES) {
            let flush: FlushMenuThemes = std::mem::transmute(entry);
            flush();
        }
    }
}

fn uxtheme_module() -> Option<HMODULE> {
    static MODULE: OnceLock<Option<usize>> = OnceLock::new();
    let raw = MODULE
        .get_or_init(|| unsafe {
            LoadLibraryW(w!("uxtheme.dll"))
                .ok()
                .map(|module| module.0 as usize)
        })
        .as_ref()
        .copied()?;
    Some(HMODULE(raw as *mut _))
}

unsafe fn uxtheme_proc(
    module: HMODULE,
    ordinal: u16,
) -> Option<unsafe extern "system" fn() -> isize> {
    unsafe { GetProcAddress(module, PCSTR::from_raw(ordinal as usize as *const u8)) }
}

/// 收敛暗色主题的文字亮度。
///
/// `foreground` 覆盖正文与输入框，`accent_foreground` 覆盖选中标签，
/// `popover_foreground` 覆盖菜单与悬停层；三者统一，避免局部仍跳出纯白。
fn soften_dark_foreground(cx: &mut App) {
    let theme = Theme::global_mut(cx);
    if !theme.mode.is_dark() {
        return;
    }

    let foreground = rgb(DARK_FOREGROUND).into();
    theme.foreground = foreground;
    theme.accent_foreground = foreground;
    theme.popover_foreground = foreground;
    theme.muted_foreground = rgb(DARK_MUTED_FOREGROUND).into();
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

    #[test]
    fn native_menu_modes_preserve_all_three_theme_intents() {
        assert_eq!(ThemePreference::System.native_app_mode(), APP_MODE_ALLOW_DARK);
        assert_eq!(ThemePreference::Light.native_app_mode(), APP_MODE_FORCE_LIGHT);
        assert_eq!(ThemePreference::Dark.native_app_mode(), APP_MODE_FORCE_DARK);
    }
}
