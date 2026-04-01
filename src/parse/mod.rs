pub mod content;
pub mod metadata;
pub mod session;
pub mod tools;

use chrono::{DateTime, Utc};

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
