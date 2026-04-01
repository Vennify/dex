use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::parse::session::SessionFile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexState {
    pub indexed_sessions: HashMap<String, SessionEntry>,
    pub last_full_index: Option<DateTime<Utc>>,
    pub tantivy_doc_count: u64,
    #[serde(default)]
    pub vector_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub size: u64,
    pub modified: DateTime<Utc>,
}

impl IndexState {
    pub fn new() -> Self {
        IndexState {
            indexed_sessions: HashMap::new(),
            last_full_index: None,
            tantivy_doc_count: 0,
            vector_count: 0,
        }
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_else(Self::new)
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)
    }

    /// Check if a session needs (re)indexing.
    pub fn needs_indexing(&self, session: &SessionFile) -> bool {
        match self.indexed_sessions.get(&session.session_id) {
            None => true,
            Some(entry) => entry.size != session.size,
        }
    }

    /// Mark a session as indexed.
    pub fn mark_indexed(&mut self, session: &SessionFile, doc_count: u64) {
        let modified = session
            .modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|d| DateTime::from_timestamp(d.as_secs() as i64, 0))
            .unwrap_or_else(Utc::now);

        self.indexed_sessions.insert(
            session.session_id.clone(),
            SessionEntry {
                size: session.size,
                modified,
            },
        );
        self.tantivy_doc_count += doc_count;
    }
}
