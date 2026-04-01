use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Session metadata from ~/.claude/usage-data/session-meta/<uuid>.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub start_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub duration_minutes: Option<f64>,
    #[serde(default)]
    pub user_message_count: Option<u64>,
    #[serde(default)]
    pub assistant_message_count: Option<u64>,
    #[serde(default)]
    pub tool_counts: Option<HashMap<String, u64>>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub first_prompt: Option<String>,
    #[serde(default)]
    pub lines_added: Option<u64>,
    #[serde(default)]
    pub lines_removed: Option<u64>,
    #[serde(default)]
    pub files_modified: Option<u64>,
}

/// Load session metadata for a given session ID.
pub fn load_session_meta(meta_dir: &Path, session_id: &str) -> Option<SessionMeta> {
    let path = meta_dir.join(format!("{session_id}.json"));
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Load all session metadata files from the meta directory.
pub fn load_all_session_meta(meta_dir: &Path) -> HashMap<String, SessionMeta> {
    let mut map = HashMap::new();
    let entries = match std::fs::read_dir(meta_dir) {
        Ok(e) => e,
        Err(_) => return map,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let session_id = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if let Ok(data) = std::fs::read_to_string(&path) {
                if let Ok(meta) = serde_json::from_str::<SessionMeta>(&data) {
                    map.insert(session_id, meta);
                }
            }
        }
    }
    map
}
