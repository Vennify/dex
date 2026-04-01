mod config;
mod embed;
mod index;
mod output;
mod parse;
mod query;

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};

use config::Config;
use embed::model::Embedder;
use index::state::IndexState;
use index::tantivy_index;
use index::vector::{VectorMeta, VectorStore};
use output::format::{self, SessionListItem, ShowFilter};
use parse::metadata;
use parse::session;
use parse::ContentType;
use tantivy::schema::Value as TantivyValue;
use query::filters::{parse_date, SearchFilters};
use query::text;

#[derive(Parser)]
#[command(name = "dex", about = "Claude Code conversation indexer and search")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Index conversation history
    Index {
        /// Full reindex from scratch
        #[arg(long)]
        full: bool,
        /// Index only one project
        #[arg(long)]
        project: Option<String>,
        /// Show index stats
        #[arg(long)]
        status: bool,
        /// Skip embedding generation (text index only)
        #[arg(long)]
        no_embed: bool,
    },
    /// Search conversations
    Search {
        /// Search query
        query: Option<String>,
        /// Exact text search only (no semantic)
        #[arg(long)]
        exact: bool,
        /// Semantic search only
        #[arg(long)]
        semantic: bool,
        /// Filter by role (user, assistant, system)
        #[arg(long)]
        role: Option<String>,
        /// Filter by tool name (Edit, Bash, etc.)
        #[arg(long)]
        tool: Option<String>,
        /// Filter by project
        #[arg(long)]
        project: Option<String>,
        /// Filter by content type (text, thinking, tool_use, tool_result)
        #[arg(long, name = "type")]
        content_type: Option<String>,
        /// Filter by file path
        #[arg(long)]
        file: Option<String>,
        /// Only show results after this date (YYYY-MM-DD)
        #[arg(long)]
        after: Option<String>,
        /// Only show results before this date (YYYY-MM-DD)
        #[arg(long)]
        before: Option<String>,
        /// Max results
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Show N surrounding messages for context
        #[arg(long)]
        context: Option<usize>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List sessions
    Sessions {
        /// Filter by project
        #[arg(long)]
        project: Option<String>,
        /// Only show sessions after this date
        #[arg(long)]
        after: Option<String>,
        /// Sort by: time (default), tokens, duration
        #[arg(long, default_value = "time")]
        sort: String,
    },
    /// Show a session's conversation
    Show {
        /// Session ID (prefix match supported)
        session_id: String,
        /// Show only user messages
        #[arg(long)]
        user: bool,
        /// Show only assistant text
        #[arg(long)]
        assistant: bool,
        /// Show only tool calls
        #[arg(long)]
        tools: bool,
        /// Show only Edit tool calls
        #[arg(long)]
        edits: bool,
        /// List all files touched
        #[arg(long)]
        files: bool,
        /// List all bash commands run
        #[arg(long)]
        commands: bool,
    },
    /// Show file history across all sessions
    File {
        /// File path (substring match)
        path: String,
        /// Show only edits
        #[arg(long)]
        edits: bool,
        /// Show only reads
        #[arg(long)]
        reads: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show statistics
    Stats {
        /// Filter by project
        #[arg(long)]
        project: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

/// Search mode derived from CLI flags.
enum SearchMode {
    Hybrid,
    Exact,
    Semantic,
}

fn main() {
    let cli = Cli::parse();
    let config = Config::new();

    match cli.command {
        Commands::Index {
            full,
            project,
            status,
            no_embed,
        } => {
            cmd_index(&config, full, project.as_deref(), status, no_embed);
        }
        Commands::Search {
            query: query_str,
            exact,
            semantic,
            role,
            tool,
            project,
            content_type,
            file,
            after,
            before,
            limit,
            context,
            json,
        } => {
            let query_str = match query_str {
                Some(q) => q,
                None => {
                    eprintln!("Error: search query is required");
                    std::process::exit(1);
                }
            };
            let filters = SearchFilters {
                role,
                tool,
                project,
                content_type,
                file_path: file,
                after: after.as_deref().and_then(parse_date),
                before: before.as_deref().and_then(parse_date),
            };
            let mode = if exact {
                SearchMode::Exact
            } else if semantic {
                SearchMode::Semantic
            } else {
                SearchMode::Hybrid
            };
            cmd_search(&config, &query_str, &filters, limit, mode, context, json);
        }
        Commands::Sessions { project, after, sort } => {
            cmd_sessions(&config, project.as_deref(), after.as_deref(), &sort);
        }
        Commands::Show {
            session_id,
            user,
            assistant,
            tools,
            edits,
            files,
            commands,
        } => {
            let filter = if user {
                ShowFilter::User
            } else if assistant {
                ShowFilter::Assistant
            } else if tools {
                ShowFilter::Tools
            } else if edits {
                ShowFilter::Edits
            } else if files {
                ShowFilter::Files
            } else if commands {
                ShowFilter::Commands
            } else {
                ShowFilter::All
            };
            cmd_show(&config, &session_id, filter);
        }
        Commands::File { path, edits, reads, json } => {
            cmd_file(&config, &path, edits, reads, json);
        }
        Commands::Stats { project, json } => {
            cmd_stats(&config, project.as_deref(), json);
        }
    }
}

fn cmd_index(config: &Config, full: bool, project_filter: Option<&str>, status: bool, no_embed: bool) {
    if let Err(e) = config.ensure_dirs() {
        eprintln!("Error creating data directories: {e}");
        std::process::exit(1);
    }

    let mut state = if full {
        IndexState::new()
    } else {
        IndexState::load(&config.state_file)
    };

    if status {
        println!("Indexed sessions: {}", state.indexed_sessions.len());
        println!("Total documents:  {}", state.tantivy_doc_count);
        println!("Total vectors:    {}", state.vector_count);
        if let Some(ref last) = state.last_full_index {
            println!("Last full index:  {}", last);
        }
        return;
    }

    // Discover sessions
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);
    let sessions: Vec<_> = all_sessions
        .into_iter()
        .filter(|s| {
            if let Some(pf) = project_filter {
                s.project.contains(pf)
            } else {
                true
            }
        })
        .collect();

    let to_index: Vec<_> = sessions
        .iter()
        .filter(|s| full || state.needs_indexing(s))
        .collect();

    if to_index.is_empty() {
        println!("Index is up to date. {} sessions indexed.", state.indexed_sessions.len());
        return;
    }

    println!("Indexing {} session(s)...", to_index.len());

    // Open tantivy
    let schema = tantivy_index::build_schema();
    let index = match tantivy_index::open_or_create(&config.tantivy_dir, &schema) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("Error opening index: {e}");
            std::process::exit(1);
        }
    };

    let mut writer = match index.writer(50_000_000) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Error creating index writer: {e}");
            std::process::exit(1);
        }
    };

    // Load embedder + vector store if embedding is enabled
    let mut embedder_and_store: Option<(Embedder, VectorStore)> = if !no_embed {
        match load_embedder_and_store(config) {
            Ok(pair) => Some(pair),
            Err(e) => {
                eprintln!("Warning: embedding disabled — {e}");
                None
            }
        }
    } else {
        None
    };

    let embedding_enabled = embedder_and_store.is_some();
    if embedding_enabled {
        eprintln!("Embedding enabled — generating vectors for each session.");
    }

    let pb = ProgressBar::new(to_index.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} sessions ({eta}) {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    let mut total_docs = 0u64;
    let mut total_vectors = 0u64;

    for (_session_idx, session_file) in to_index.iter().enumerate() {
        pb.set_message(format!(
            "| {}docs {}vecs",
            total_docs, total_vectors,
        ));

        // Delete old docs for this session if re-indexing
        if state.indexed_sessions.contains_key(&session_file.session_id) {
            tantivy_index::delete_session(&mut writer, &schema, &session_file.session_id);
            if let Some((_, ref mut store)) = embedder_and_store {
                store.remove_session(&session_file.session_id);
            }
        }

        let records = session::parse_session(session_file);
        let count = match tantivy_index::index_records(&writer, &schema, &records) {
            Ok(c) => c,
            Err(e) => {
                pb.println(format!("Warning: failed to index session {}: {e}", session_file.session_id));
                pb.inc(1);
                continue;
            }
        };

        // Generate embeddings for eligible records
        if let Some((ref mut embedder, ref mut store)) = embedder_and_store {
            let embeddable: Vec<_> = records.iter().filter(|r| should_embed(r)).collect();
            for record in &embeddable {
                match embedder.embed_chunked(&record.content) {
                    Ok(chunks) => {
                        for (chunk, embedding) in chunks {
                            let meta = VectorMeta {
                                session_id: record.session_id.clone(),
                                message_id: record.message_id.clone(),
                                chunk_index: chunk.index,
                            };
                            if let Err(e) = store.add(&embedding, meta) {
                                pb.println(format!("Warning: vector add error: {e}"));
                            } else {
                                total_vectors += 1;
                            }
                        }
                    }
                    Err(e) => {
                        pb.println(format!("Warning: embed error for {}: {e}", record.message_id));
                    }
                }
            }
        }

        state.mark_indexed(session_file, count);
        total_docs += count;
        pb.inc(1);
    }

    if let Err(e) = writer.commit() {
        eprintln!("Error committing index: {e}");
        std::process::exit(1);
    }

    // Save vector store
    if let Some((_, ref store)) = embedder_and_store {
        if let Err(e) = store.save() {
            eprintln!("Warning: failed to save vector store: {e}");
        }
        state.vector_count = store.len() as u64;
    }

    pb.finish_and_clear();

    if let Err(e) = state.save(&config.state_file) {
        eprintln!("Warning: failed to save index state: {e}");
    }

    println!(
        "Indexed {} documents, {} vectors from {} sessions. Total: {} sessions tracked.",
        total_docs,
        total_vectors,
        to_index.len(),
        state.indexed_sessions.len(),
    );
}

/// Determine if a record should be embedded (per spec: not tool results).
fn should_embed(record: &parse::Record) -> bool {
    match record.content_type {
        ContentType::ToolResult => false,
        _ => !record.content.is_empty(),
    }
}

fn load_embedder_and_store(config: &Config) -> Result<(Embedder, VectorStore), String> {
    let models_dir = config.data_dir.join("models");
    let (model_path, tokenizer_path) = embed::download::ensure_model(&models_dir)?;
    let embedder = Embedder::load(&model_path, &tokenizer_path)?;
    let store = VectorStore::open(&config.data_dir)?;
    Ok((embedder, store))
}

fn cmd_search(
    config: &Config,
    query_str: &str,
    filters: &SearchFilters,
    limit: usize,
    mode: SearchMode,
    context: Option<usize>,
    json_output: bool,
) {
    let schema = tantivy_index::build_schema();
    let index = match tantivy_index::open_or_create(&config.tantivy_dir, &schema) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("Error opening index: {e}");
            std::process::exit(1);
        }
    };

    let results = match mode {
        SearchMode::Exact => {
            text::search(&index, &schema, query_str, filters, limit)
                .unwrap_or_else(|e| { eprintln!("Search error: {e}"); std::process::exit(1); })
        }
        SearchMode::Semantic => {
            let (mut embedder, store) = match load_embedder_and_store(config) {
                Ok(pair) => pair,
                Err(e) => { eprintln!("Error loading embedder: {e}"); std::process::exit(1); }
            };
            let sem_results = query::semantic::search(&mut embedder, &store, query_str, limit)
                .unwrap_or_else(|e| { eprintln!("Semantic search error: {e}"); std::process::exit(1); });
            let reader = index.reader().expect("reader");
            let searcher = reader.searcher();
            sem_results.into_iter().filter_map(|sem| {
                lookup_by_message_id(&index, &schema, &searcher, &sem.message_id).map(|r| {
                    query::text::SearchResult { score: 1.0 - sem.distance, ..r }
                })
            }).collect()
        }
        SearchMode::Hybrid => {
            match load_embedder_and_store(config) {
                Ok((mut embedder, store)) => {
                    query::hybrid::search(&index, &schema, &mut embedder, &store, query_str, filters, limit)
                        .unwrap_or_else(|e| { eprintln!("Hybrid search error: {e}"); std::process::exit(1); })
                }
                Err(_) => {
                    eprintln!("Note: no vector index found, falling back to text search.");
                    text::search(&index, &schema, query_str, filters, limit)
                        .unwrap_or_else(|e| { eprintln!("Search error: {e}"); std::process::exit(1); })
                }
            }
        }
    };

    if json_output {
        format::print_search_results_json(&results);
    } else if let Some(ctx_n) = context {
        // Show surrounding messages for each result
        let all_sessions = session::discover_sessions(&config.claude_projects_dir);
        format::print_search_results_with_context(&results, &all_sessions, ctx_n);
    } else {
        format::print_search_results(&results);
    }
}

/// Look up a document by message_id in tantivy.
fn lookup_by_message_id(
    _index: &tantivy::Index,
    schema: &tantivy::schema::Schema,
    searcher: &tantivy::Searcher,
    message_id: &str,
) -> Option<query::text::SearchResult> {
    use tantivy::schema::IndexRecordOption;
    let message_id_field = schema.get_field("message_id").ok()?;
    let term = tantivy::Term::from_field_text(message_id_field, message_id);
    let q = tantivy::query::TermQuery::new(term, IndexRecordOption::Basic);
    let top = searcher
        .search(&q, &tantivy::collector::TopDocs::with_limit(1))
        .ok()?;
    let (_, doc_addr) = top.into_iter().next()?;
    let doc: tantivy::TantivyDocument = searcher.doc(doc_addr).ok()?;

    Some(query::text::SearchResult {
        session_id: get_field_text(&doc, schema, "session_id"),
        message_id: get_field_text(&doc, schema, "message_id"),
        project: get_field_text(&doc, schema, "project"),
        role: get_field_text(&doc, schema, "role"),
        content_type: get_field_text(&doc, schema, "content_type"),
        tool_name: get_field_text(&doc, schema, "tool_name"),
        file_path: get_field_text(&doc, schema, "file_path"),
        content: get_field_text(&doc, schema, "content"),
        score: 0.0,
        sequence: 0,
    })
}

fn get_field_text(doc: &tantivy::TantivyDocument, schema: &tantivy::schema::Schema, name: &str) -> String {
    let field = schema.get_field(name).unwrap();
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn cmd_sessions(config: &Config, project_filter: Option<&str>, after: Option<&str>, sort: &str) {
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);
    let meta_map = metadata::load_all_session_meta(&config.claude_session_meta_dir);

    let after_dt = after.and_then(parse_date);

    let mut items: Vec<SessionListItem> = all_sessions
        .into_iter()
        .filter(|s| {
            if let Some(pf) = project_filter {
                s.project.contains(pf)
            } else {
                true
            }
        })
        .filter_map(|s| {
            let meta = meta_map.get(&s.session_id).cloned();

            // Apply date filter
            if let Some(after_dt) = after_dt {
                if let Some(ref m) = meta {
                    if let Some(ref st) = m.start_time {
                        if *st < after_dt {
                            return None;
                        }
                    }
                }
            }

            Some(SessionListItem {
                session_id: s.session_id,
                project: s.project,
                start_time: meta.as_ref().and_then(|m| m.start_time.map(|t| t.format("%Y-%m-%d %H:%M").to_string())),
                first_prompt: meta.as_ref().and_then(|m| m.first_prompt.clone()),
                meta,
            })
        })
        .collect();

    // Sort
    match sort {
        "tokens" => {
            items.sort_by(|a, b| {
                let ta = a.meta.as_ref().and_then(|m| m.input_tokens).unwrap_or(0);
                let tb = b.meta.as_ref().and_then(|m| m.input_tokens).unwrap_or(0);
                tb.cmp(&ta)
            });
        }
        "duration" => {
            items.sort_by(|a, b| {
                let da = a.meta.as_ref().and_then(|m| m.duration_minutes).unwrap_or(0.0);
                let db = b.meta.as_ref().and_then(|m| m.duration_minutes).unwrap_or(0.0);
                db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        _ => {
            // Default: sort by time, most recent first
            items.sort_by(|a, b| {
                let ta = a.start_time.as_deref().unwrap_or("");
                let tb = b.start_time.as_deref().unwrap_or("");
                tb.cmp(ta)
            });
        }
    }

    format::print_session_list(&items);
}

fn cmd_show(config: &Config, session_id_prefix: &str, filter: ShowFilter) {
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);

    // Find session by prefix match
    let matching: Vec<_> = all_sessions
        .iter()
        .filter(|s| s.session_id.starts_with(session_id_prefix))
        .collect();

    match matching.len() {
        0 => {
            eprintln!("No session found matching '{session_id_prefix}'");
            std::process::exit(1);
        }
        1 => {
            let records = session::parse_session(matching[0]);
            format::print_session_show(&records, filter);
        }
        n => {
            eprintln!("Ambiguous prefix '{session_id_prefix}' matches {n} sessions:");
            for s in &matching[..n.min(5)] {
                eprintln!("  {} ({})", s.session_id, s.project);
            }
            std::process::exit(1);
        }
    }
}

fn cmd_file(config: &Config, path_query: &str, edits_only: bool, reads_only: bool, json_output: bool) {
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);

    let mut file_records: Vec<format::FileHistoryItem> = Vec::new();

    for sf in &all_sessions {
        let records = session::parse_session(sf);
        for record in &records {
            let file_path = match &record.file_path {
                Some(fp) => fp,
                None => continue,
            };
            if !file_path.contains(path_query) {
                continue;
            }
            // Apply edits/reads filter
            if edits_only && record.tool_name.as_deref() != Some("Edit") {
                continue;
            }
            if reads_only && record.tool_name.as_deref() != Some("Read") {
                continue;
            }
            file_records.push(format::FileHistoryItem {
                session_id: record.session_id.clone(),
                project: record.project.clone(),
                tool_name: record.tool_name.clone().unwrap_or_default(),
                file_path: file_path.clone(),
                content: record.content.clone(),
                timestamp: record.timestamp,
            });
        }
    }

    if json_output {
        format::print_file_history_json(&file_records);
    } else {
        format::print_file_history(&file_records);
    }
}

fn cmd_stats(config: &Config, project_filter: Option<&str>, json_output: bool) {
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);
    let meta_map = metadata::load_all_session_meta(&config.claude_session_meta_dir);

    let sessions: Vec<_> = all_sessions
        .into_iter()
        .filter(|s| {
            if let Some(pf) = project_filter {
                s.project.contains(pf)
            } else {
                true
            }
        })
        .collect();

    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut total_duration = 0.0f64;
    let mut total_messages = 0u64;
    let mut tool_counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let mut files_touched: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut projects: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut earliest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut latest: Option<chrono::DateTime<chrono::Utc>> = None;

    for sf in &sessions {
        projects.insert(sf.project.clone());
        if let Some(meta) = meta_map.get(&sf.session_id) {
            total_input_tokens += meta.input_tokens.unwrap_or(0);
            total_output_tokens += meta.output_tokens.unwrap_or(0);
            total_duration += meta.duration_minutes.unwrap_or(0.0);
            total_messages += meta.user_message_count.unwrap_or(0) + meta.assistant_message_count.unwrap_or(0);
            if let Some(ref tc) = meta.tool_counts {
                for (tool, count) in tc {
                    *tool_counts.entry(tool.clone()).or_default() += count;
                }
            }
            if let Some(st) = meta.start_time {
                if earliest.is_none() || st < earliest.unwrap() {
                    earliest = Some(st);
                }
                if latest.is_none() || st > latest.unwrap() {
                    latest = Some(st);
                }
            }
        }
        // Count files by parsing (only if not too many sessions)
        if sessions.len() <= 200 {
            let records = session::parse_session(sf);
            for r in &records {
                if let Some(ref fp) = r.file_path {
                    files_touched.insert(fp.clone());
                }
            }
        }
    }

    let stats = format::StatsOutput {
        session_count: sessions.len(),
        project_count: projects.len(),
        message_count: total_messages,
        input_tokens: total_input_tokens,
        output_tokens: total_output_tokens,
        total_duration_minutes: total_duration,
        tool_counts,
        files_touched: files_touched.len(),
        earliest: earliest.map(|t| t.format("%Y-%m-%d").to_string()),
        latest: latest.map(|t| t.format("%Y-%m-%d").to_string()),
    };

    if json_output {
        format::print_stats_json(&stats);
    } else {
        format::print_stats(&stats);
    }
}
