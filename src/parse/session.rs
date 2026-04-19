use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::content::extract_content_blocks;
use super::{ContentType, DerivedMeta, ParsedSession, Record, Role, SessionKind, TeamMemberRef};

/// A discovered session file on disk.
#[derive(Debug, Clone)]
pub struct SessionFile {
    /// Stable identifier used everywhere (tantivy, state.json, `dex show`).
    /// For regular/teammate sessions this is the UUID. For subagents it is
    /// the file stem (e.g. `agent-a1ec0f8ce3cedb281`).
    pub session_id: String,
    pub project: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: std::time::SystemTime,
    pub kind: SessionKind,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub parent_session_uuid: Option<String>,
}

/// Discover all session JSONL files under the Claude projects directory,
/// including nested `<parent>/subagents/*.jsonl`.
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
            // Top-level JSONL — regular or teammate
            if file_path.extension().is_some_and(|e| e == "jsonl") {
                if let Some(sf) = make_top_level_session(&file_path, &project_name) {
                    sessions.push(sf);
                }
                continue;
            }
            // <uuid>/ directory — may contain a subagents/ subdir
            if file_path.is_dir() {
                let parent_uuid = file_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let subagents_dir = file_path.join("subagents");
                if subagents_dir.is_dir() {
                    collect_subagents(&subagents_dir, &project_name, &parent_uuid, &mut sessions);
                }
            }
        }
    }

    sessions
}

fn make_top_level_session(path: &Path, project: &str) -> Option<SessionFile> {
    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let meta = std::fs::metadata(path).ok()?;
    let kind = peek_kind(path);
    Some(SessionFile {
        session_id,
        project: project.to_string(),
        path: path.to_path_buf(),
        size: meta.len(),
        modified: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
        kind,
        agent_id: None,
        agent_type: None,
        parent_session_uuid: None,
    })
}

/// Cheap first-line peek to classify a top-level session at discovery time.
/// Looks for the `teamName` / `agentName` markers that tag teammate files.
fn peek_kind(path: &Path) -> SessionKind {
    let Ok(file) = File::open(path) else {
        return SessionKind::Regular;
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    // Up to ~8 lines — teamName usually lands on the first few assistant/user lines.
    for _ in 0..8 {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Err(_) => break,
            Ok(_) => {}
        }
        if line.contains("\"teamName\"")
            || line.contains("\"agentName\"")
            || line.contains("<teammate-message")
        {
            return SessionKind::Teammate;
        }
    }
    SessionKind::Regular
}

fn collect_subagents(
    subagents_dir: &Path,
    project: &str,
    parent_uuid: &str,
    out: &mut Vec<SessionFile>,
) {
    let entries = match std::fs::read_dir(subagents_dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e == "jsonl") {
            continue;
        }
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let stem = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // `agent-<agentId>` → agent_id = <agentId>
        let agent_id = stem.strip_prefix("agent-").map(String::from);
        // Look for sibling <stem>.meta.json
        let meta_path = path.with_extension("meta.json");
        let agent_type = if meta_path.exists() {
            std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|data| serde_json::from_str::<Value>(&data).ok())
                .and_then(|v| v.get("agentType").and_then(|a| a.as_str()).map(String::from))
        } else {
            None
        };

        out.push(SessionFile {
            session_id: stem,
            project: project.to_string(),
            path,
            size: meta.len(),
            modified: meta.modified().unwrap_or(std::time::UNIX_EPOCH),
            kind: SessionKind::Subagent,
            agent_id,
            agent_type,
            parent_session_uuid: Some(parent_uuid.to_string()),
        });
    }
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
                meta: initial_meta(session),
            };
        }
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut meta = initial_meta(session);
    let mut sequence: u64 = 0;
    let mut first_user_block_seen = false;

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

        // Teammate / agent classification — populate once from first seen tags
        if meta.team_name.is_none() {
            if let Some(tn) = json.get("teamName").and_then(|v| v.as_str()) {
                meta.team_name = Some(tn.to_string());
            }
        }
        if meta.agent_name.is_none() {
            if let Some(an) = json.get("agentName").and_then(|v| v.as_str()) {
                meta.agent_name = Some(an.to_string());
            }
        }

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

        match role {
            Role::User => meta.user_message_count += 1,
            Role::Assistant => meta.assistant_message_count += 1,
            Role::System => {}
        }

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

        // Team-member capture: any Agent tool_use with team_name in input
        // marks this session as a team-lead for that (team_name, agent_name).
        if role == Role::Assistant {
            if let Some(Value::Array(arr)) = content_value {
                for block in arr {
                    if block.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                        continue;
                    }
                    if block.get("name").and_then(|v| v.as_str()) != Some("Agent") {
                        continue;
                    }
                    let input = match block.get("input") {
                        Some(v) => v,
                        None => continue,
                    };
                    let team = match input.get("team_name").and_then(|v| v.as_str()) {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => continue,
                    };
                    let name = input
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    meta.team_members_spawned.push(TeamMemberRef {
                        team_name: team,
                        agent_name: name,
                    });
                }
            }
        }

        let content_value = match content_value {
            Some(v) => v,
            None => continue,
        };

        let blocks = extract_content_blocks(role, content_value);

        for (i, block) in blocks.into_iter().enumerate() {
            // Detect teammate by first user text block pattern.
            if !first_user_block_seen
                && role == Role::User
                && block.content_type == ContentType::Text
            {
                first_user_block_seen = true;
                if block.content.trim_start().starts_with("<teammate-message") {
                    // Only promote to Teammate if we haven't already been
                    // forced to Subagent by the path-based classifier.
                    if meta.kind == SessionKind::Regular {
                        meta.kind = SessionKind::Teammate;
                    }
                }
            }

            if meta.first_prompt.is_none()
                && role == Role::User
                && block.content_type == ContentType::Text
                && !block.content.trim().is_empty()
            {
                meta.first_prompt = Some(first_line(&block.content, 240));
            }

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

    // If we still see it as regular but the teamName field was present,
    // treat as teammate (covers files where the first user block doesn't
    // have the <teammate-message> wrapper).
    if meta.kind == SessionKind::Regular
        && meta.team_name.is_some()
        && meta.agent_name.is_some()
    {
        meta.kind = SessionKind::Teammate;
    }

    ParsedSession { records, meta }
}

fn initial_meta(session: &SessionFile) -> DerivedMeta {
    DerivedMeta {
        kind: session.kind,
        agent_id: session.agent_id.clone(),
        agent_type: session.agent_type.clone(),
        parent_session_uuid: session.parent_session_uuid.clone(),
        ..DerivedMeta::default()
    }
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
