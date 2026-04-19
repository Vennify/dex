pub mod content;
pub mod metadata;
pub mod session;
pub mod tool_events;
pub mod tools;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A normalized record ready for indexing. One per content block.
#[derive(Debug, Clone)]
pub struct Record {
    pub session_id: String,
    pub message_id: String,
    pub project: String,
    pub role: Role,
    pub content_type: ContentType,
    pub tool_name: Option<String>,
    pub file_path: Option<String>,
    pub command: Option<String>,
    pub content: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Text,
    Thinking,
    ToolUse,
    ToolResult,
}

impl ContentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContentType::Text => "text",
            ContentType::Thinking => "thinking",
            ContentType::ToolUse => "tool_use",
            ContentType::ToolResult => "tool_result",
        }
    }
}

impl std::fmt::Display for ContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Session-level metadata derived directly from the JSONL. Replaces the
/// now-defunct session-meta JSON cache (Claude Code stopped writing those
/// on 2026-03-22), while remaining compatible with older sessions.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DerivedMeta {
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub first_prompt: Option<String>,
    pub user_message_count: u64,
    pub assistant_message_count: u64,
    pub tool_counts: HashMap<String, u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub files_modified: HashSet<String>,
    /// The working directory stamped on user messages (first seen).
    pub cwd: Option<String>,
}

impl DerivedMeta {
    pub fn duration_minutes(&self) -> Option<f64> {
        let s = self.start_time?;
        let e = self.end_time?;
        let secs = (e - s).num_seconds();
        if secs < 0 {
            None
        } else {
            Some(secs as f64 / 60.0)
        }
    }
}

/// Parsed session: records + derived session-level meta, computed in a single pass.
pub struct ParsedSession {
    pub records: Vec<Record>,
    pub meta: DerivedMeta,
}
