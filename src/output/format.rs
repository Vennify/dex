use colored::Colorize;

use crate::parse::metadata::SessionMeta;
use crate::query::text::SearchResult;

/// Format and print search results to the terminal.
pub fn print_search_results(results: &[SearchResult]) {
    if results.is_empty() {
        println!("{}", "No results found.".dimmed());
        return;
    }

    println!("{} result(s)\n", results.len().to_string().bold());

    for (i, result) in results.iter().enumerate() {
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

        // Session / project info
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

        // Content preview (first 3 lines)
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
