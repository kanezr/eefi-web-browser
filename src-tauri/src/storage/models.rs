use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub id: i64,
    pub title: String,
    pub url: String,
    pub folder_id: Option<i64>,
    pub folder_name: Option<String>,
    pub on_bar: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkFolder {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub visited_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OmniboxSuggestion {
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEntry {
    pub id: i64,
    pub url: String,
    pub path: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct SessionTab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub position: usize,
    pub active: bool,
    pub group_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionGroup {
    pub id: String,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub revision: u64,
    pub tabs: Vec<SessionTab>,
    pub groups: Vec<SessionGroup>,
}
