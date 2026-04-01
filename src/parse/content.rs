use serde_json::Value;

use super::tools::extract_tool_fields;
use super::{ContentType, Role};

/// Parsed content block before it becomes a full Record.
pub struct ContentBlock {
    pub content_type: ContentType,
    pub tool_name: Option<String>,
    pub file_path: Option<String>,
    pub command: Option<String>,
    pub content: String,
}

/// Strip <system-reminder>...</system-reminder> tags from user message text.
fn strip_system_reminders(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find("<system-reminder>") {
        result.push_str(&remaining[..start]);
        if let Some(end) = remaining[start..].find("</system-reminder>") {
            remaining = &remaining[start + end + "</system-reminder>".len()..];
        } else {
            // Unclosed tag — skip to end
            remaining = "";
        }
    }
    result.push_str(remaining);
    result
}

/// Extract content blocks from a message's content field.
/// `content_value` is `message.content` which can be a string or array of blocks.
pub fn extract_content_blocks(role: Role, content_value: &Value) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();

    match content_value {
        Value::String(s) => {
            let text = if role == Role::User {
                strip_system_reminders(s)
            } else {
                s.clone()
            };
            let text = text.trim().to_string();
            if !text.is_empty() {
                blocks.push(ContentBlock {
                    content_type: ContentType::Text,
                    tool_name: None,
                    file_path: None,
                    command: None,
                    content: text,
                });
            }
        }
        Value::Array(arr) => {
            for block in arr {
                let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match block_type {
                    "text" => {
                        let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                        let text = if role == Role::User {
                            strip_system_reminders(text)
                        } else {
                            text.to_string()
                        };
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            blocks.push(ContentBlock {
                                content_type: ContentType::Text,
                                tool_name: None,
                                file_path: None,
                                command: None,
                                content: text,
                            });
                        }
                    }
                    "thinking" => {
                        let text = block.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                        if !text.is_empty() {
                            blocks.push(ContentBlock {
                                content_type: ContentType::Thinking,
                                tool_name: None,
                                file_path: None,
                                command: None,
                                content: text.to_string(),
                            });
                        }
                    }
                    "tool_use" => {
                        let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
                        let input = block.get("input").unwrap_or(&Value::Null);
                        let (content, file_path, command) = extract_tool_fields(name, input);
                        blocks.push(ContentBlock {
                            content_type: ContentType::ToolUse,
                            tool_name: Some(name.to_string()),
                            file_path,
                            command,
                            content,
                        });
                    }
                    "tool_result" => {
                        let text = extract_tool_result_text(block);
                        if !text.is_empty() {
                            // Truncate large tool results (keep first N lines)
                            let truncated = truncate_by_lines(&text, 50);
                            blocks.push(ContentBlock {
                                content_type: ContentType::ToolResult,
                                tool_name: None,
                                file_path: None,
                                command: None,
                                content: truncated,
                            });
                        }
                    }
                    _ => {} // skip unknown block types
                }
            }
        }
        _ => {}
    }

    blocks
}

/// Extract text from a tool_result block. Content can be string or array of content blocks.
fn extract_tool_result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(text);
                }
            }
            parts.join("\n")
        }
        _ => {
            // Sometimes output is directly in the block
            block.get("output").and_then(|v| v.as_str()).unwrap_or("").to_string()
        }
    }
}

/// Truncate text to at most `max_lines` lines.
fn truncate_by_lines(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        text.to_string()
    } else {
        let mut result: String = lines[..max_lines].join("\n");
        result.push_str(&format!("\n... ({} more lines truncated)", lines.len() - max_lines));
        result
    }
}
