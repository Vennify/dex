//! Raw tool-use event extraction, for commands like `dex recover` that need
//! the full untruncated tool input JSON (e.g. full `old_string` / `new_string`
//! on every Edit). The main `parse_session` path synthesizes a short display
//! summary; this path preserves the original input verbatim.

use std::fs::File;
use std::io::{BufRead, BufReader};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

use super::session::SessionFile;

#[derive(Debug, Clone, Serialize)]
pub struct ToolUseEvent {
    pub session_id: String,
    pub project: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub tool: String,
    /// Extracted file path (for Edit/Write/Read/Grep) for filtering
    /// convenience. The full input is always in `input`.
    pub file_path: Option<String>,
    pub input: Value,
}

/// Filter for which events to keep.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Keep only events whose tool name is in this set. Empty = any tool.
    pub tools: Vec<String>,
    /// Keep only events whose extracted file_path contains this substring.
    pub file_substr: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

impl EventFilter {
    pub fn matches(&self, event: &ToolUseEvent) -> bool {
        if !self.tools.is_empty() && !self.tools.iter().any(|t| t == &event.tool) {
            return false;
        }
        if let Some(ref needle) = self.file_substr {
            match &event.file_path {
                Some(fp) if fp.contains(needle) => {}
                _ => return false,
            }
        }
        if let Some(since) = self.since {
            if let Some(ts) = event.timestamp {
                if ts < since {
                    return false;
                }
            }
        }
        if let Some(until) = self.until {
            if let Some(ts) = event.timestamp {
                if ts > until {
                    return false;
                }
            }
        }
        true
    }
}

/// Walk a session's JSONL and emit every tool_use block (with full raw input).
pub fn extract_tool_uses(session: &SessionFile, filter: &EventFilter) -> Vec<ToolUseEvent> {
    let file = match File::open(&session.path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let json: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type != "assistant" {
            continue;
        }

        let timestamp = json
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());

        let content = match json
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| json.get("content"))
        {
            Some(c) => c,
            None => continue,
        };

        let blocks = match content.as_array() {
            Some(b) => b,
            None => continue,
        };

        for block in blocks {
            if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                continue;
            }
            let tool = block
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if tool.is_empty() {
                continue;
            }
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let file_path = extract_file_path(&tool, &input);

            let event = ToolUseEvent {
                session_id: session.session_id.clone(),
                project: session.project.clone(),
                timestamp,
                tool,
                file_path,
                input,
            };

            if filter.matches(&event) {
                events.push(event);
            }
        }
    }

    events
}

fn extract_file_path(tool: &str, input: &Value) -> Option<String> {
    match tool {
        "Edit" | "Write" | "Read" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(String::from),
        "Grep" => input.get("path").and_then(|v| v.as_str()).map(String::from),
        _ => None,
    }
}
