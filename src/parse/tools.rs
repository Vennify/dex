use serde_json::Value;

/// Truncate a string to at most `max` bytes, respecting char boundaries.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Extract tool-specific fields from a tool_use input object.
/// Returns (synthesized_content, file_path, command).
pub fn extract_tool_fields(name: &str, input: &Value) -> (String, Option<String>, Option<String>) {
    match name {
        "Edit" => {
            let file_path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("unknown");
            let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
            let new = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
            let summary = if old.len() + new.len() > 200 {
                format!("Edit {file_path}: replaced {} chars with {} chars", old.len(), new.len())
            } else {
                format!("Edit {file_path}: {old} → {new}")
            };
            (summary, Some(file_path.to_string()), None)
        }
        "Read" => {
            let file_path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("unknown");
            (format!("Read {file_path}"), Some(file_path.to_string()), None)
        }
        "Write" => {
            let file_path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("unknown");
            (format!("Write {file_path}"), Some(file_path.to_string()), None)
        }
        "Bash" => {
            let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            (format!("Bash: {command}"), None, Some(command.to_string()))
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
            (format!("Grep {pattern} in {path}"), Some(path.to_string()), None)
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            (format!("Glob {pattern}"), None, None)
        }
        "Agent" => {
            let subagent_type = input.get("subagent_type").and_then(|v| v.as_str()).unwrap_or("general");
            let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            let truncated = truncate_str(prompt, 200);
            (format!("Agent ({subagent_type}): {truncated}"), None, None)
        }
        other => {
            // Generic tool — just record name + a snippet of input
            let snippet = serde_json::to_string(input).unwrap_or_default();
            let truncated = truncate_str(&snippet, 150);
            (format!("{other}: {truncated}"), None, None)
        }
    }
}
