use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::content::extract_content_blocks;
use super::{Record, Role};

/// A discovered session file on disk.
#[derive(Debug, Clone)]
pub struct SessionFile {
    pub session_id: String,
    pub project: String,
    pub path: std::path::PathBuf,
    pub size: u64,
    pub modified: std::time::SystemTime,
}

/// Discover all session JSONL files under the Claude projects directory.
pub fn discover_sessions(projects_dir: &Path) -> Vec<SessionFile> {
    let mut sessions = Vec::new();

    let entries = match std::fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(_) => return sessions,
    };

    for project_entry in entries.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let project_name = project_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let files = match std::fs::read_dir(&project_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for file_entry in files.flatten() {
            let file_path = file_entry.path();
            if file_path.extension().is_some_and(|e| e == "jsonl") {
                let session_id = file_path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let meta = match file_entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                sessions.push(SessionFile {
                    session_id,
                    project: project_name.clone(),
                    path: file_path,
                    size: meta.len(),
                    modified: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                });
            }
        }
    }

    sessions
}

/// Parse a single session JSONL file into Records.
pub fn parse_session(session: &SessionFile) -> Vec<Record> {
    let file = match File::open(&session.path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut sequence: u64 = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue, // skip malformed lines
        };
        if line.trim().is_empty() {
            continue;
        }

        let json: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // skip unparseable lines
        };

        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

        // Skip types we don't index
        match msg_type {
            "user" | "assistant" | "system" => {}
            _ => continue, // progress, file-history-snapshot, etc.
        }

        let role = match msg_type {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            _ => continue,
        };

        // Extract timestamp
        let timestamp = json
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());

        // Get the message content — could be at json.message.content or json.content
        let content_value = json
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| json.get("content"));

        let content_value = match content_value {
            Some(v) => v,
            None => continue,
        };

        let blocks = extract_content_blocks(role, content_value);

        for (i, block) in blocks.into_iter().enumerate() {
            let message_id = format!("{}-{}-{}", session.session_id, sequence, i);
            records.push(Record {
                session_id: session.session_id.clone(),
                message_id,
                project: session.project.clone(),
                role,
                content_type: block.content_type,
                tool_name: block.tool_name,
                file_path: block.file_path,
                command: block.command,
                content: block.content,
                timestamp,
                sequence,
            });
        }

        sequence += 1;
    }

    records
}
