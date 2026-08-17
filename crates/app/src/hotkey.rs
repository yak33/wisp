//! 唤起快捷键的词法：标签串 ⇄ [`HotKey`] 的互转与合法性校验。
//!
//! 这里刻意让**存盘串 = 显示串 = 解析输入**三者同形（如 `"Ctrl+Alt+Space"`）：
//! `HotKey: FromStr` 的词法（`+` 分隔、大小写不敏感）与 gpui `Keystroke` 的
//! 小写键名天然对齐，故无需维护一张 `Code` 映射表。唯一的例外是 Windows 键——
//! 界面按 Windows 习惯显示 `Win`，`HotKey` 只认 `Super`，仅在解析一步做替换。

use std::str::FromStr as _;

use global_hotkey::hotkey::HotKey;
use gpui::Keystroke;

/// 默认候选键，按序降级注册。Alt+Space 大概率被 uTools 等工具占用。
pub(crate) const DEFAULT_CANDIDATES: [&str; 3] = ["Alt+Space", "Ctrl+Alt+Space", "Alt+`"];

/// 页内已占用的组合。全局快捷键不分焦点一律触发，若与页内键位重合，
/// 用户在应用内按该键会同时触发唤起切换与页内动作。
const RESERVED: [&str; 13] = [
    "Ctrl+1",
    "Ctrl+2",
    "Ctrl+P",
    "Ctrl+N",
    "Ctrl+E",
    "Ctrl+S",
    "Ctrl+Enter",
    "Ctrl+,",
    "Alt+1",
    "Alt+2",
    "Alt+3",
    "Alt+4",
    "Alt+5",
];

/// 录制被拒的原因，文案直接呈现给用户。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rejection {
    /// 只按下了修饰键，还没按主键
    ModifierOnly,
    /// 缺少 Ctrl / Alt / Win——裸键与 Shift+键会吞掉正常输入
    NeedsModifier,
    /// 与页内快捷键冲突
    Reserved,
    /// 该键不在 `HotKey` 词法内
    Unsupported,
}

impl Rejection {
    pub fn message(self) -> &'static str {
        match self {
            Self::ModifierOnly => "请再按一个主键",
            Self::NeedsModifier => "需带 Ctrl / Alt / Win 修饰键",
            Self::Reserved => "该组合已被页内快捷键占用",
            Self::Unsupported => "不支持该按键",
        }
    }
}

/// 把标签串解析为可注册的 [`HotKey`]。
pub(crate) fn parse(label: &str) -> Option<HotKey> {
    // 界面显示 Win，HotKey 词法只认 Super
    HotKey::from_str(&label.replace("Win", "Super")).ok()
}

/// 从一次按键构造标签串，顺带完成合法性校验。
///
/// 修饰键顺序固定为 Ctrl → Alt → Shift → Win，保证同一组合只有一种写法，
/// 落盘串因此可直接用于比较。
pub(crate) fn from_keystroke(keystroke: &Keystroke) -> Result<String, Rejection> {
    let modifiers = &keystroke.modifiers;
    let key = keystroke.key.as_str();

    // gpui 会为修饰键本身发送按键事件，录制时先放过等主键
    if matches!(
        key,
        "control" | "alt" | "shift" | "platform" | "function" | "ctrl" | "cmd" | "win" | "super"
    ) {
        return Err(Rejection::ModifierOnly);
    }

    // Shift 不计入：Shift+A 就是大写 A，占用它会吞掉正常输入
    if !(modifiers.control || modifiers.alt || modifiers.platform) {
        return Err(Rejection::NeedsModifier);
    }

    let mut label = String::new();
    for (active, token) in [
        (modifiers.control, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
        (modifiers.platform, "Win"),
    ] {
        if active {
            label.push_str(token);
            label.push('+');
        }
    }
    label.push_str(&main_key_token(key));

    if RESERVED.contains(&label.as_str()) {
        return Err(Rejection::Reserved);
    }
    // 能注册的前提是能解析——不可识别的键在此拦下
    parse(&label).map(|_| label).ok_or(Rejection::Unsupported)
}

/// 主键的显示写法：单字符照原样（大写），具名键首字母大写。
fn main_key_token(key: &str) -> String {
    if key.chars().count() == 1 {
        return key.to_uppercase();
    }
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use gpui::Modifiers;

    use super::*;

    /// 造一次按键。`mods` 按 (ctrl, alt, shift, win) 给。
    fn stroke(mods: (bool, bool, bool, bool), key: &str) -> Keystroke {
        Keystroke {
            modifiers: Modifiers {
                control: mods.0,
                alt: mods.1,
                shift: mods.2,
                platform: mods.3,
                ..Default::default()
            },
            key: key.into(),
            ..Default::default()
        }
    }

    /// 默认候选键必须都能解析——它们是启动时的降级链，解析失败等于无法唤起。
    #[test]
    fn default_candidates_all_parse() {
        for label in DEFAULT_CANDIDATES {
            assert!(parse(label).is_some(), "候选键 {label} 应可解析");
        }
    }

    /// 录制产出的标签必须能反向解析回 HotKey——存盘串与解析输入同形的前提。
    #[test]
    fn recorded_labels_round_trip() {
        let cases = [
            ((true, false, false, false), "a", "Ctrl+A"),
            ((false, true, false, false), "space", "Alt+Space"),
            ((true, true, false, false), "space", "Ctrl+Alt+Space"),
            ((false, true, false, false), "`", "Alt+`"),
            ((true, false, true, false), "q", "Ctrl+Shift+Q"),
            ((false, false, false, true), "v", "Win+V"),
            ((false, true, false, false), "f1", "Alt+F1"),
            ((true, false, false, false), "up", "Ctrl+Up"),
        ];

        for (mods, key, expected) in cases {
            let label = from_keystroke(&stroke(mods, key)).expect("应接受该组合");
            assert_eq!(label, expected);
            assert!(parse(&label).is_some(), "{label} 应可解析回 HotKey");
        }
    }

    /// 修饰键顺序归一：同一组合无论按下顺序，标签唯一。
    #[test]
    fn modifier_order_is_canonical() {
        let label = from_keystroke(&stroke((true, true, true, true), "k")).expect("应接受");
        assert_eq!(label, "Ctrl+Alt+Shift+Win+K");
    }

    #[test]
    fn rejects_illegal_combinations() {
        let cases = [
            // 裸键与纯 Shift 会吞掉正常输入
            (
                ((false, false, false, false), "a"),
                Rejection::NeedsModifier,
            ),
            (((false, false, true, false), "a"), Rejection::NeedsModifier),
            // 还没按主键
            (
                ((true, false, false, false), "control"),
                Rejection::ModifierOnly,
            ),
            (
                ((false, true, false, false), "alt"),
                Rejection::ModifierOnly,
            ),
            // 与页内键位冲突
            (((true, false, false, false), "1"), Rejection::Reserved),
            (((false, true, false, false), "5"), Rejection::Reserved),
            (((true, false, false, false), "s"), Rejection::Reserved),
        ];

        for ((mods, key), expected) in cases {
            assert_eq!(
                from_keystroke(&stroke(mods, key)),
                Err(expected),
                "键 {key}"
            );
        }
    }

    /// 页内保留键必须自身可解析，否则 RESERVED 表写错了也发现不了。
    #[test]
    fn reserved_entries_are_wellformed() {
        for label in RESERVED {
            assert!(parse(label).is_some(), "保留键 {label} 应可解析");
        }
    }
}
