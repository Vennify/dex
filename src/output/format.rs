use std::collections::HashMap;

use chrono::{DateTime, Utc};
use colored::Colorize;

use crate::parse::metadata::SessionMeta;
use crate::parse::session::{self, SessionFile};
use crate::query::text::SearchResult;

/// Format and print search results to the terminal.
pub fn print_search_results(results: &[SearchResult]) {
    if results.is_empty() {
        println!("{}", "No results found.".dimmed());
        return;
    }

    println!("{} result(s)\n", results.len().to_string().bold());

    for (i, result) in results.iter().enumerate() {
        print_single_result(i, result);
    }
}

fn print_single_result(i: usize, result: &SearchResult) {
    let role_colored = match result.role.as_str() {
        "user" => result.role.green(),
        "assistant" => result.role.blue(),
        "system" => result.role.yellow(),
        _ => result.role.normal(),
    };

    let header = format!(
        "{}  {}  {}  {}",
        format!("[{}]", i + 1).dimmed(),
        role_colored,
        result.content_type.dimmed(),
        format!("score:{:.2}", result.score).dimmed(),
    );
    println!("{}", header);

    println!(
        "  {} {} {}",
        "session:".dimmed(),
        &result.session_id[..8.min(result.session_id.len())],
        format!("({})", result.project).dimmed(),
    );

    if !result.tool_name.is_empty() {
        println!("  {} {}", "tool:".dimmed(), result.tool_name.cyan());
    }
    if !result.file_path.is_empty() {
        println!("  {} {}", "file:".dimmed(), result.file_path);
    }

    let preview: String = result
        .content
        .lines()
        .take(3)
        .collect::<Vec<_>>()
        .join("\n  ");
    println!("  {}", preview);

    if result.content.lines().count() > 3 {
        println!("  {}", "...".dimmed());
    }
    println!();
}

/// Print search results with N surrounding messages of context.
pub fn print_search_results_with_context(
    results: &[SearchResult],
    all_sessions: &[SessionFile],
    context_n: usize,
) {
    if results.is_empty() {
        println!("{}", "No results found.".dimmed());
        return;
    }

    println!("{} result(s)\n", results.len().to_string().bold());

    // Cache parsed sessions to avoid re-parsing
    let mut session_cache: HashMap<String, Vec<crate::parse::Record>> = HashMap::new();

    for (i, result) in results.iter().enumerate() {
        print_single_result(i, result);

        // Find the session and show surrounding messages
        if context_n > 0 {
            let records = session_cache
                .entry(result.session_id.clone())
                .or_insert_with(|| {
                    all_sessions
                        .iter()
                        .find(|s| s.session_id == result.session_id)
                        .map(|sf| session::parse_session(sf))
                        .unwrap_or_default()
                });

            // Find the matching record by sequence
            if let Some(pos) = records.iter().position(|r| r.message_id == result.message_id) {
                let start = pos.saturating_sub(context_n);
                let end = (pos + context_n + 1).min(records.len());

                println!("  {} context ({} messages around match):", "---".dimmed(), context_n);
                for j in start..end {
                    let r = &records[j];
                    let marker = if j == pos { ">>" } else { "  " };
                    let role_str = match r.role {
                        crate::parse::Role::User => "user".green(),
                        crate::parse::Role::Assistant => "assistant".blue(),
                        crate::parse::Role::System => "system".yellow(),
                    };
                    let preview: String = r.content.lines().next().unwrap_or("").chars().take(100).collect();
                    println!("  {} {} {}", marker, role_str, preview.dimmed());
                }
                println!();
            }
        }
    }
}

/// Print search results as JSON.
pub fn print_search_results_json(results: &[SearchResult]) {
    let json_results: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "session_id": r.session_id,
                "message_id": r.message_id,
                "project": r.project,
                "role": r.role,
                "content_type": r.content_type,
                "tool_name": r.tool_name,
                "file_path": r.file_path,
                "content": r.content,
                "score": r.score,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&json_results).unwrap());
}

/// Format and print session list.
pub fn print_session_list(sessions: &[SessionListItem]) {
    if sessions.is_empty() {
        println!("{}", "No sessions found.".dimmed());
        return;
    }

    println!("{} session(s)\n", sessions.len().to_string().bold());

    for session in sessions {
        let time_str = session
            .start_time
            .as_deref()
            .unwrap_or("unknown time");

        let prompt_preview = session
            .first_prompt
            .as_deref()
            .unwrap_or("(no prompt)")
            .chars()
            .take(80)
            .collect::<String>();

        println!(
            "{}  {}  {}",
            session.session_id[..8.min(session.session_id.len())].bold(),
            time_str.dimmed(),
            format!("({})", session.project).dimmed(),
        );
        println!("  {}", prompt_preview);

        if let Some(ref meta) = session.meta {
            let mut stats = Vec::new();
            if let Some(d) = meta.duration_minutes {
                stats.push(format!("{:.0}min", d));
            }
            if let Some(t) = meta.input_tokens {
                stats.push(format!("{}tok in", t));
            }
            if let Some(t) = meta.output_tokens {
                stats.push(format!("{}tok out", t));
            }
            if !stats.is_empty() {
                println!("  {}", stats.join(" | ").dimmed());
            }
        }
        println!();
    }
}

/// A display-ready session list item.
pub struct SessionListItem {
    pub session_id: String,
    pub project: String,
    pub start_time: Option<String>,
    pub first_prompt: Option<String>,
    pub meta: Option<SessionMeta>,
}

/// Format and print a full session conversation.
pub fn print_session_show(records: &[crate::parse::Record], show_filter: ShowFilter) {
    if records.is_empty() {
        println!("{}", "Session not found or empty.".dimmed());
        return;
    }

    for record in records {
        match show_filter {
            ShowFilter::All => {}
            ShowFilter::User => {
                if record.role != crate::parse::Role::User {
                    continue;
                }
            }
            ShowFilter::Assistant => {
                if record.role != crate::parse::Role::Assistant
                    || record.content_type != crate::parse::ContentType::Text
                {
                    continue;
                }
            }
            ShowFilter::Tools => {
                if record.content_type != crate::parse::ContentType::ToolUse {
                    continue;
                }
            }
            ShowFilter::Edits => {
                if record.tool_name.as_deref() != Some("Edit") {
                    continue;
                }
            }
            ShowFilter::Files => {
                if record.file_path.is_none() {
                    continue;
                }
                println!("{}", record.file_path.as_deref().unwrap());
                continue;
            }
            ShowFilter::Commands => {
                if record.command.is_none() {
                    continue;
                }
                println!("{}", record.command.as_deref().unwrap());
                continue;
            }
        }

        let role_colored = match record.role {
            crate::parse::Role::User => "user".green().bold(),
            crate::parse::Role::Assistant => "assistant".blue().bold(),
            crate::parse::Role::System => "system".yellow().bold(),
        };

        if record.content_type == crate::parse::ContentType::ToolUse {
            let tool = record.tool_name.as_deref().unwrap_or("?");
            println!("{} [{}]", role_colored, tool.cyan());
        } else {
            println!("{}", role_colored);
        }

        println!("{}\n", record.content);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ShowFilter {
    All,
    User,
    Assistant,
    Tools,
    Edits,
    Files,
    Commands,
}

// --- File history ---

pub struct FileHistoryItem {
    pub session_id: String,
    pub project: String,
    pub tool_name: String,
    pub file_path: String,
    pub content: String,
    pub timestamp: Option<DateTime<Utc>>,
}

pub fn print_file_history(items: &[FileHistoryItem]) {
    if items.is_empty() {
        println!("{}", "No file history found.".dimmed());
        return;
    }

    println!("{} record(s)\n", items.len().to_string().bold());

    for item in items {
        let time_str = item
            .timestamp
            .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "unknown".to_string());

        println!(
            "{}  {}  {}  {}",
            &item.session_id[..8.min(item.session_id.len())].bold(),
            item.tool_name.cyan(),
            time_str.dimmed(),
            format!("({})", item.project).dimmed(),
        );

        let preview: String = item.content.lines().take(2).collect::<Vec<_>>().join("\n  ");
        println!("  {}", preview);
        println!();
    }
}

pub fn print_file_history_json(items: &[FileHistoryItem]) {
    let json: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "session_id": item.session_id,
                "project": item.project,
                "tool_name": item.tool_name,
                "file_path": item.file_path,
                "content": item.content,
                "timestamp": item.timestamp.map(|t| t.to_rfc3339()),
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}

// --- Stats ---

pub struct StatsOutput {
    pub session_count: usize,
    pub project_count: usize,
    pub message_count: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_duration_minutes: f64,
    pub tool_counts: HashMap<String, u64>,
    pub files_touched: usize,
    pub earliest: Option<String>,
    pub latest: Option<String>,
}

pub fn print_stats(stats: &StatsOutput) {
    println!("{}", "dex stats".bold());
    println!("  Sessions:       {}", stats.session_count);
    println!("  Projects:       {}", stats.project_count);
    println!("  Messages:       {}", stats.message_count);
    println!("  Input tokens:   {}", format_number(stats.input_tokens));
    println!("  Output tokens:  {}", format_number(stats.output_tokens));
    println!("  Total duration: {:.0} hours", stats.total_duration_minutes / 60.0);

    if stats.files_touched > 0 {
        println!("  Files touched:  {}", stats.files_touched);
    }

    if let (Some(e), Some(l)) = (&stats.earliest, &stats.latest) {
        println!("  Date range:     {} to {}", e, l);
    }

    if !stats.tool_counts.is_empty() {
        println!("\n  {}", "Tool usage:".bold());
        let mut sorted: Vec<_> = stats.tool_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (tool, count) in sorted {
            println!("    {:<12} {}", tool, count);
        }
    }
}

pub fn print_stats_json(stats: &StatsOutput) {
    let json = serde_json::json!({
        "session_count": stats.session_count,
        "project_count": stats.project_count,
        "message_count": stats.message_count,
        "input_tokens": stats.input_tokens,
        "output_tokens": stats.output_tokens,
        "total_duration_minutes": stats.total_duration_minutes,
        "tool_counts": stats.tool_counts,
        "files_touched": stats.files_touched,
        "earliest": stats.earliest,
        "latest": stats.latest,
    });
    println!("{}", serde_json::to_string_pretty(&json).unwrap());
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
