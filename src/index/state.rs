use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::parse::session::SessionFile;
use crate::parse::DerivedMeta;

/// Bump whenever the tantivy schema or on-disk derived-meta layout
/// changes in an incompatible way. Mismatch triggers a full reindex.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexState {
    pub indexed_sessions: HashMap<String, SessionEntry>,
    pub last_full_index: Option<DateTime<Utc>>,
    pub tantivy_doc_count: u64,
    #[serde(default)]
    pub vector_count: u64,
    #[serde(default)]
    pub last_index: Option<DateTime<Utc>>,
    #[serde(default)]
    pub schema_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub size: u64,
    pub modified: DateTime<Utc>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub meta: Option<DerivedMeta>,
    /// Whether embeddings were generated for this session. A `--no-embed`
    /// run leaves this false so a later embedding pass can fill them in
    /// without a full reindex.
    #[serde(default)]
    pub embedded: bool,
}

impl IndexState {
    pub fn new() -> Self {
        IndexState {
            indexed_sessions: HashMap::new(),
            last_full_index: None,
            tantivy_doc_count: 0,
            vector_count: 0,
            last_index: None,
            schema_version: SCHEMA_VERSION,
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

    /// Mark a session as indexed, storing its derived meta.
    pub fn mark_indexed(
        &mut self,
        session: &SessionFile,
        doc_count: u64,
        meta: DerivedMeta,
        embedded: bool,
    ) {
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
                project: Some(session.project.clone()),
                meta: Some(meta),
                embedded,
            },
        );
        self.tantivy_doc_count += doc_count;
    }

    /// True if this session's JSONL has been text-indexed but is missing
    /// vector embeddings (e.g. indexed under `--no-embed`).
    pub fn needs_embedding(&self, session: &SessionFile) -> bool {
        match self.indexed_sessions.get(&session.session_id) {
            Some(entry) if entry.size == session.size => !entry.embedded,
            _ => false,
        }
    }
}
