//! 轻量本地配置：`key=value` 逐行存放，随写随存。
//!
//! 刻意不引 serde/toml——配置项一只手数得过来，手写解析足够且零依赖。
//!
//! 以 [`Global`] 单实例存在：标题栏、设置页、根视图都要读写同一份配置，
//! 各自持有副本会互相覆盖（一方写入时会把另一方的旧快照整体落盘）。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use gpui::{App, Global};

pub(crate) struct Config {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl Global for Config {}

/// 读取配置项。
pub(crate) fn get(key: &str, cx: &App) -> Option<String> {
    cx.try_global::<Config>()
        .and_then(|config| config.get(key))
        .map(str::to_owned)
}

/// 写入配置项并立即落盘。
pub(crate) fn set(key: &str, value: &str, cx: &mut App) {
    if cx.has_global::<Config>() {
        cx.global_mut::<Config>().set(key, value);
    }
}

impl Config {
    /// 从 `path` 读取配置；文件不存在或损坏时按空配置启动。
    pub fn load(path: &Path) -> Self {
        let values = fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
            .collect();
        Self {
            path: path.to_path_buf(),
            values,
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// 写入并立即落盘。失败静默——配置丢失只是回到默认值，不值得惊动用户。
    pub fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
        let text: String = self
            .values
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect();
        _ = fs::write(&self.path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 落盘 → 重新加载应原样取回；缺失键回落 None 交由调用方定默认值。
    #[test]
    fn values_round_trip_through_config_file() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 Unix Epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "wisp-config-{}-{unique}.cfg",
            std::process::id()
        ));

        let mut config = Config::load(&path);
        config.set("theme", "dark");
        config.set("last_page", "clipboard");

        let reloaded = Config::load(&path);
        assert_eq!(reloaded.get("theme"), Some("dark"));
        assert_eq!(reloaded.get("last_page"), Some("clipboard"));
        assert_eq!(reloaded.get("absent"), None);

        _ = fs::remove_file(path);
    }
}
