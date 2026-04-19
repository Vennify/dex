use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::content::extract_content_blocks;
use super::{ContentType, DerivedMeta, ParsedSession, Record, Role};

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

/// Parse a single session JSONL file into records only (legacy entry point).
pub fn parse_session(session: &SessionFile) -> Vec<Record> {
    parse_session_full(session).records
}

/// Parse a single session JSONL file, returning records + derived session meta.
pub fn parse_session_full(session: &SessionFile) -> ParsedSession {
    let file = match File::open(&session.path) {
        Ok(f) => f,
        Err(_) => {
            return ParsedSession {
                records: Vec::new(),
                meta: DerivedMeta::default(),
            };
        }
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut meta = DerivedMeta::default();
    let mut sequence: u64 = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        let json: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let role = match msg_type {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "system" => Role::System,
            _ => continue,
        };

        let timestamp = json
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<DateTime<Utc>>().ok());

        if let Some(ts) = timestamp {
            meta.start_time = Some(meta.start_time.map_or(ts, |s| s.min(ts)));
            meta.end_time = Some(meta.end_time.map_or(ts, |e| e.max(ts)));
        }

        if meta.cwd.is_none() {
            if let Some(cwd) = json.get("cwd").and_then(|v| v.as_str()) {
                meta.cwd = Some(cwd.to_string());
            }
        }

        // Per-role counters
        match role {
            Role::User => meta.user_message_count += 1,
            Role::Assistant => meta.assistant_message_count += 1,
            Role::System => {}
        }

        // Assistant-side token usage lives in message.usage
        if role == Role::Assistant {
            if let Some(usage) = json.get("message").and_then(|m| m.get("usage")) {
                if let Some(n) = usage.get("input_tokens").and_then(|v| v.as_u64()) {
                    meta.input_tokens += n;
                }
                if let Some(n) = usage.get("output_tokens").and_then(|v| v.as_u64()) {
                    meta.output_tokens += n;
                }
            }
        }

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
            // First-prompt: first non-empty user text block
            if meta.first_prompt.is_none()
                && role == Role::User
                && block.content_type == ContentType::Text
                && !block.content.trim().is_empty()
            {
                meta.first_prompt = Some(first_line(&block.content, 240));
            }

            // Tool counts
            if block.content_type == ContentType::ToolUse {
                if let Some(ref name) = block.tool_name {
                    *meta.tool_counts.entry(name.clone()).or_default() += 1;
                    if matches!(name.as_str(), "Edit" | "Write") {
                        if let Some(ref fp) = block.file_path {
                            meta.files_modified.insert(fp.clone());
                        }
                    }
                }
            }

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

    ParsedSession { records, meta }
}

fn first_line(s: &str, max_chars: usize) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    let truncated: String = line.chars().take(max_chars).collect();
    if line.chars().count() > max_chars {
        format!("{truncated}…")
    } else {
        truncated
    }
}
