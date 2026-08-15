//! 备忘快贴的领域模型。

/// 一条备忘片段。`tags` 由存储层聚合而来，顺序与录入顺序一致。
#[derive(Debug, Clone)]
pub struct Memo {
    pub id: i64,
    pub content: String,
    /// 备注：列表里显示在内容下方的小字，用于回忆"这段是干嘛的"
    pub note: String,
    pub tags: Vec<String>,
    pub preview: String,
    pub updated_at: i64,
}

/// 新建或更新一条备忘。`id` 为空表示新建。
#[derive(Debug, Clone, Default)]
pub struct MemoDraft {
    pub id: Option<i64>,
    pub content: String,
    pub note: String,
    pub tags: Vec<String>,
}

/// 侧栏的标签筛选条件。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TagFilter {
    #[default]
    All,
    Untagged,
    Named(String),
}

/// 侧栏条目：标签名与其下的备忘数量。
#[derive(Debug, Clone)]
pub struct TagSummary {
    pub name: String,
    pub count: i64,
}

/// 解析用户输入的标签串：中英文逗号、顿号与空白均可分隔，去重且保序。
pub fn parse_tags(raw: &str) -> Vec<String> {
    raw.split([',', '，', '、', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .fold(Vec::new(), |mut tags, tag| {
            if !tags.iter().any(|existing: &String| existing == tag) {
                tags.push(tag.to_string());
            }
            tags
        })
}

#[cfg(test)]
mod tests {
    use super::parse_tags;

    #[test]
    fn tags_are_split_deduped_and_ordered() {
        assert_eq!(
            parse_tags(" 工作，工作、私活  笔记 ,, "),
            vec!["工作", "私活", "笔记"]
        );
        assert!(parse_tags("   ").is_empty());
    }
}
