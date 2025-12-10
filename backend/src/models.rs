use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

// --- App 数据模型 ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub id: String,         // 文件完整路径
    pub title: String,      // 显示名称
    pub subtitle: String,   // 副标题(路径)
    
    // --- 搜索优化字段 ---
    pub title_pinyin: String,   // 全拼: "weixin"
    pub title_acronym: String,  // 首字母: "wx"
    // -------------------

    #[serde(skip)]
    pub score: i64,         // 动态计算的分数
    pub action_type: String,
    pub action_data: String,
    pub use_count: u32,     // 使用次数
    pub last_used: Option<DateTime<Utc>>,
    pub file_type: String,
}

impl SearchResult {
    // 构造函数更新，传入预计算的拼音
    pub fn new(path: String, name: String, f_type: String, pinyin: String, acronym: String) -> Self {
        Self {
            id: path.clone(),
            title: name,
            subtitle: path.clone(),
            score: 0,
            action_type: if f_type == "Folder" { "folder".into() } else { "file".into() },
            action_data: path,
            use_count: 0,
            last_used: None,
            file_type: f_type,
            title_pinyin: pinyin,
            title_acronym: acronym,
        }
    }
}

// --- 用户习惯记录 ---
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserHabits {
    // Key: 搜索词, Value: { AppID: 点击次数 }
    pub history: HashMap<String, HashMap<String, u32>>,
}

impl UserHabits {
    pub fn record(&mut self, query: &str, app_id: &str) {
        let query = query.trim().to_lowercase();
        if query.is_empty() { return; }
        let entry = self.history.entry(query).or_insert_with(HashMap::new);
        let count = entry.entry(app_id.to_string()).or_insert(0);
        *count += 1;
    }

    pub fn get_weight(&self, query: &str, app_id: &str) -> i64 {
        let query = query.trim().to_lowercase();
        if let Some(apps) = self.history.get(&query) {
            if let Some(&count) = apps.get(app_id) {
                // 历史记录权重极大(1000分)，保证用过的就在最上面
                return count as i64 * 1000; 
            }
        }
        0
    }
}

// --- 应用设置 ---
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub max_results: usize,
    pub enable_autostart: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            max_results: 100,
            enable_autostart: false,
        }
    }
}