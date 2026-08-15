//! 剪贴板条目的领域模型。

/// 条目类型。第一里程碑只落地文本，枚举为图像/文件预留位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    Text = 0,
    Image = 1,
    Files = 2,
}

impl ClipKind {
    pub fn from_i64(value: i64) -> Self {
        match value {
            1 => Self::Image,
            2 => Self::Files,
            _ => Self::Text,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "文本",
            Self::Image => "图像",
            Self::Files => "文件",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Clip {
    pub id: i64,
    pub kind: ClipKind,
    pub content: String,
    /// 列表展示用的单行摘要（入库时生成，避免每帧截断大文本）
    pub preview: String,
    pub pinned: bool,
    /// Unix 毫秒
    pub created_at: i64,
    pub char_count: i64,
}

/// 内容指纹：FNV-1a 64。仅用于去重预筛，命中后仍比对原文，
/// 因此哈希碰撞不会造成误判，只会多一次字符串比较。
pub(crate) fn fingerprint(content: &str) -> i64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let hash = content
        .as_bytes()
        .iter()
        .fold(OFFSET, |hash, &byte| (hash ^ byte as u64).wrapping_mul(PRIME));
    hash as i64
}

/// 生成列表摘要：取首个非空行，按字符截断。
pub(crate) fn make_preview(content: &str) -> String {
    const MAX_CHARS: usize = 120;

    let first_line = content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");

    let mut preview: String = first_line.chars().take(MAX_CHARS).collect();
    if first_line.chars().count() > MAX_CHARS || content.lines().count() > 1 {
        preview.push('…');
    }
    preview
}
