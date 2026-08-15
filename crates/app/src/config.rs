//! 轻量本地配置：`key=value` 逐行存放，随写随存。
//!
//! 刻意不引 serde/toml——配置项一只手数得过来，手写解析足够且零依赖。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct Config {
    path: PathBuf,
    values: BTreeMap<String, String>,
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
