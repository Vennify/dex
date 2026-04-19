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

/// What sort of Claude Code session this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    Regular,
    /// Native Claude Code subagent (spawned via Agent tool, isSidechain=true).
    /// Lives under `<parent>/subagents/agent-<agentId>.jsonl`.
    Subagent,
    /// Wigwam-orchestrated teammate (spawned via Agent tool with
    /// `team_name` in input, runs in a parallel Claude Code session).
    /// Has its own top-level JSONL with `teamName` / `agentName` fields.
    Teammate,
}

impl SessionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionKind::Regular => "regular",
            SessionKind::Subagent => "subagent",
            SessionKind::Teammate => "teammate",
        }
    }
}

/// A teammate-member reference emitted by a team-lead when it calls
/// Agent{team_name:..., name:...}. Captured during parse so we can later
/// resolve teammate → team-lead links.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMemberRef {
    pub team_name: String,
    pub agent_name: String,
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

    // Session-kind fields ------------------------------------------------
    #[serde(default)]
    pub kind: SessionKind,
    /// Native subagent id (e.g. "a1ec0f8ce3cedb281"). Subagents only.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Subagent type from sibling meta.json (e.g. "Explore"). Subagents only.
    #[serde(default)]
    pub agent_type: Option<String>,
    /// Subagent's parent session UUID (the dir name). Subagents only.
    #[serde(default)]
    pub parent_session_uuid: Option<String>,
    /// Wigwam team name. Teammates only.
    #[serde(default)]
    pub team_name: Option<String>,
    /// Teammate agent name (e.g. "p1-migration"). Teammates only.
    #[serde(default)]
    pub agent_name: Option<String>,
    /// Team-lead session id, resolved post-index via team-member map.
    /// Teammates only.
    #[serde(default)]
    pub team_lead_session_id: Option<String>,
    /// Teammates this session spawned (via Agent{team_name:...}).
    /// Regular sessions acting as team-leads populate this.
    #[serde(default)]
    pub team_members_spawned: Vec<TeamMemberRef>,
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
