mod config;
mod embed;
mod index;
mod output;
mod parse;
mod query;
mod service;

use std::collections::HashMap;

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};

use config::Config;
use embed::model::Embedder;
use index::state::IndexState;
use index::tantivy_index;
use index::vector::{VectorMeta, VectorStore};
use output::format::{self, SessionListItem, ShowFilter};
use parse::metadata;
use parse::session::{self, SessionFile};
use parse::ContentType;
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
        /// Index only one project (substring match)
        #[arg(long)]
        project: Option<String>,
        /// Show index stats
        #[arg(long)]
        status: bool,
        /// Skip embedding generation (text index only)
        #[arg(long)]
        no_embed: bool,
        /// Stay resident and reindex on JSONL changes (debounced)
        #[arg(long)]
        watch: bool,
    },
    /// Manage the scheduled reindex (systemd/launchd/cron)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Search conversations
    Search {
        /// Search query (optional when filters are set)
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
        /// Filter by project (substring/prefix match against project dir)
        #[arg(long)]
        project: Option<String>,
        /// Filter by content type (text, thinking, tool_use, tool_result)
        #[arg(long, name = "type")]
        content_type: Option<String>,
        /// Filter by file path (substring)
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
        /// Filter by project (substring)
        #[arg(long)]
        project: Option<String>,
        /// Only show sessions after this date
        #[arg(long)]
        after: Option<String>,
        /// Sort by: time (default), tokens, duration
        #[arg(long, default_value = "time")]
        sort: String,
    },
    /// List indexed projects with session counts
    Projects,
    /// Show a session's conversation
    #[command(alias = "edits")]
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
        /// Restrict to records touching this file (substring match)
        #[arg(long)]
        file: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Show file history across all sessions
    #[command(alias = "log")]
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
    /// Emit every untruncated Edit/Write tool input that touched a file.
    /// Default output is JSON suitable for piping into a replay script.
    Recover {
        /// File path substring to match (e.g. "relay_client.rs")
        path: String,
        /// Limit to a specific session (prefix match)
        #[arg(long)]
        session: Option<String>,
        /// Limit to a specific project (substring match)
        #[arg(long)]
        project: Option<String>,
        /// Limit to a specific tool (default: Edit + Write)
        #[arg(long)]
        tool: Option<String>,
        /// Only include events after this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,
        /// Only include events before this date (YYYY-MM-DD)
        #[arg(long)]
        until: Option<String>,
        /// Output format: json (default) or human
        #[arg(long, default_value = "json")]
        format: String,
    },
    /// Show statistics
    Stats {
        /// Filter by project (substring)
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

#[derive(Subcommand)]
enum ServiceAction {
    /// Install a scheduled reindex (systemd user / launchd / cron)
    Install {
        /// How often to run: "hourly", "daily", "30m", "2h", ...
        #[arg(long, default_value = "hourly")]
        interval: String,
    },
    /// Uninstall the scheduled reindex
    Uninstall,
    /// Show status of the scheduled reindex
    Status,
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
            watch,
        } => {
            if watch {
                cmd_watch(&config, project.as_deref(), no_embed);
            } else {
                cmd_index(&config, full, project.as_deref(), status, no_embed);
            }
        }
        Commands::Service { action } => {
            cmd_service(action);
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
            let all_sessions = session::discover_sessions(&config.claude_projects_dir);
            let project_dirs = resolve_project_dirs(&all_sessions, project.as_deref());
            if project.is_some() && project_dirs.is_empty() {
                eprintln!(
                    "No indexed project matches '{}'. Try `dex projects` to list them.",
                    project.as_deref().unwrap()
                );
                std::process::exit(1);
            }
            let filters = SearchFilters {
                role,
                tool,
                project_dirs,
                content_type,
                file_path: file,
                after: after.as_deref().and_then(parse_date),
                before: before.as_deref().and_then(parse_date),
            };
            let query_str = match query_str {
                Some(q) => q,
                None => {
                    if !filters.any_set() {
                        eprintln!("Error: provide a search query or at least one filter (--tool, --file, --project, ...)");
                        std::process::exit(1);
                    }
                    String::new()
                }
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
        Commands::Sessions {
            project,
            after,
            sort,
        } => {
            cmd_sessions(&config, project.as_deref(), after.as_deref(), &sort);
        }
        Commands::Projects => {
            cmd_projects(&config);
        }
        Commands::Show {
            session_id,
            user,
            assistant,
            tools,
            edits,
            files,
            commands,
            file,
            json,
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
            cmd_show(&config, &session_id, filter, file.as_deref(), json);
        }
        Commands::File {
            path,
            edits,
            reads,
            json,
        } => {
            cmd_file(&config, &path, edits, reads, json);
        }
        Commands::Recover {
            path,
            session,
            project,
            tool,
            since,
            until,
            format,
        } => {
            cmd_recover(
                &config,
                &path,
                session.as_deref(),
                project.as_deref(),
                tool.as_deref(),
                since.as_deref(),
                until.as_deref(),
                &format,
            );
        }
        Commands::Stats { project, json } => {
            cmd_stats(&config, project.as_deref(), json);
        }
    }
}

fn cmd_index(
    config: &Config,
    full: bool,
    project_filter: Option<&str>,
    status: bool,
    no_embed: bool,
) {
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
        if let Some(ref last) = state.last_index {
            println!("Last index run:   {}", last.format("%Y-%m-%d %H:%M:%S UTC"));
        }
        if let Some(ref last) = state.last_full_index {
            println!("Last full index:  {}", last.format("%Y-%m-%d %H:%M:%S UTC"));
        }
        let with_meta = state
            .indexed_sessions
            .values()
            .filter(|e| e.meta.is_some())
            .count();
        println!(
            "With derived meta: {}/{}",
            with_meta,
            state.indexed_sessions.len()
        );
        return;
    }

    let all_sessions = session::discover_sessions(&config.claude_projects_dir);
    let sessions: Vec<_> = all_sessions
        .into_iter()
        .filter(|s| match project_filter {
            Some(pf) => s.project.contains(pf),
            None => true,
        })
        .collect();

    let to_index: Vec<_> = sessions
        .iter()
        .filter(|s| full || state.needs_indexing(s))
        .collect();

    // Meta backfill: sessions already indexed (tantivy) but missing derived meta.
    let to_backfill: Vec<_> = sessions
        .iter()
        .filter(|s| {
            if to_index.iter().any(|x| x.session_id == s.session_id) {
                return false;
            }
            state
                .indexed_sessions
                .get(&s.session_id)
                .map(|e| e.meta.is_none())
                .unwrap_or(false)
        })
        .collect();

    if to_index.is_empty() && to_backfill.is_empty() {
        println!(
            "Index is up to date. {} sessions indexed, {} vectors.",
            state.indexed_sessions.len(),
            state.vector_count
        );
        return;
    }

    if !to_index.is_empty() {
        println!("Indexing {} session(s)...", to_index.len());
    }

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

    let mut embedder_and_store: Option<(Embedder, VectorStore)> = if !no_embed {
        match load_embedder_and_store(config) {
            Ok(pair) => {
                eprintln!("Embeddings enabled.");
                Some(pair)
            }
            Err(e) => {
                eprintln!("Error: failed to load embedder: {e}");
                eprintln!("  Semantic search will not work until this is fixed.");
                eprintln!("  Re-run with --no-embed to skip embeddings explicitly.");
                std::process::exit(2);
            }
        }
    } else {
        eprintln!("Embeddings skipped (--no-embed).");
        None
    };

    let pb = ProgressBar::new((to_index.len() + to_backfill.len()) as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} sessions ({eta}) {msg}")
            .unwrap()
            .progress_chars("=>-"),
    );

    let mut total_docs = 0u64;
    let mut total_vectors = 0u64;

    for session_file in &to_index {
        pb.set_message(format!("| {}docs {}vecs", total_docs, total_vectors));

        if state.indexed_sessions.contains_key(&session_file.session_id) {
            tantivy_index::delete_session(&mut writer, &schema, &session_file.session_id);
            if let Some((_, ref mut store)) = embedder_and_store {
                store.remove_session(&session_file.session_id);
            }
        }

        let parsed = session::parse_session_full(session_file);

        let count = match tantivy_index::index_records(&writer, &schema, &parsed.records) {
            Ok(c) => c,
            Err(e) => {
                pb.println(format!(
                    "Warning: failed to index session {}: {e}",
                    session_file.session_id
                ));
                pb.inc(1);
                continue;
            }
        };

        if let Some((ref mut embedder, ref mut store)) = embedder_and_store {
            let embeddable: Vec<_> = parsed.records.iter().filter(|r| should_embed(r)).collect();
            let mut embed_errors = 0u64;
            let mut add_errors = 0u64;
            for record in &embeddable {
                match embedder.embed_chunked(&record.content) {
                    Ok(chunks) => {
                        for (chunk, embedding) in chunks {
                            let meta = VectorMeta {
                                session_id: record.session_id.clone(),
                                message_id: record.message_id.clone(),
                                chunk_index: chunk.index,
                            };
                            match store.add(&embedding, meta) {
                                Ok(_) => total_vectors += 1,
                                Err(e) => {
                                    add_errors += 1;
                                    if add_errors <= 3 {
                                        eprintln!("vector add error: {e}");
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        embed_errors += 1;
                        if embed_errors <= 3 {
                            pb.println(format!(
                                "embed error for {}: {e}",
                                record.message_id
                            ));
                        }
                    }
                }
            }
            if embed_errors > 0 || add_errors > 0 {
                eprintln!(
                    "session {}: {} embed errors, {} add errors ({} embeddable records)",
                    session_file.session_id,
                    embed_errors,
                    add_errors,
                    embeddable.len(),
                );
            }
        }

        state.mark_indexed(session_file, count, parsed.meta);
        total_docs += count;
        pb.inc(1);
    }

    if !to_backfill.is_empty() {
        pb.println(format!(
            "Backfilling derived meta for {} session(s)...",
            to_backfill.len()
        ));
        for session_file in &to_backfill {
            let parsed = session::parse_session_full(session_file);
            if let Some(entry) = state.indexed_sessions.get_mut(&session_file.session_id) {
                entry.meta = Some(parsed.meta);
                entry.project = Some(session_file.project.clone());
            }
            pb.inc(1);
        }
    }

    if let Err(e) = writer.commit() {
        eprintln!("Error committing index: {e}");
        std::process::exit(1);
    }

    if let Some((_, ref store)) = embedder_and_store {
        if let Err(e) = store.save() {
            eprintln!("Warning: failed to save vector store: {e}");
        }
        state.vector_count = store.len() as u64;
    }

    state.last_index = Some(chrono::Utc::now());
    if full {
        state.last_full_index = state.last_index;
    }

    pb.finish_and_clear();

    if let Err(e) = state.save(&config.state_file) {
        eprintln!("Warning: failed to save index state: {e}");
    }

    println!(
        "Indexed {} docs, {} vectors across {} sessions. Total: {} sessions / {} vectors.",
        total_docs,
        total_vectors,
        to_index.len(),
        state.indexed_sessions.len(),
        state.vector_count,
    );
}

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
        SearchMode::Exact => text::search(&index, &schema, query_str, filters, limit)
            .unwrap_or_else(|e| {
                eprintln!("Search error: {e}");
                std::process::exit(1);
            }),
        SearchMode::Semantic => {
            let (mut embedder, store) = match load_embedder_and_store(config) {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("Semantic search unavailable: {e}");
                    eprintln!("Run `dex index` to build embeddings.");
                    std::process::exit(1);
                }
            };
            if store.len() == 0 {
                eprintln!("Semantic search unavailable: no vectors indexed. Run `dex index`.");
                std::process::exit(1);
            }
            if query_str.trim().is_empty() {
                eprintln!("Semantic search requires a query string.");
                std::process::exit(1);
            }
            let sem_results = query::semantic::search(&mut embedder, &store, query_str, limit)
                .unwrap_or_else(|e| {
                    eprintln!("Semantic search error: {e}");
                    std::process::exit(1);
                });
            let reader = index.reader().expect("reader");
            let searcher = reader.searcher();
            sem_results
                .into_iter()
                .filter_map(|sem| {
                    lookup_by_message_id(&index, &schema, &searcher, &sem.message_id).map(|r| {
                        query::text::SearchResult {
                            score: 1.0 - sem.distance,
                            ..r
                        }
                    })
                })
                .collect()
        }
        SearchMode::Hybrid => match load_embedder_and_store(config) {
            Ok((mut embedder, store)) if store.len() > 0 && !query_str.trim().is_empty() => {
                query::hybrid::search(
                    &index, &schema, &mut embedder, &store, query_str, filters, limit,
                )
                .unwrap_or_else(|e| {
                    eprintln!("Hybrid search error: {e}");
                    std::process::exit(1);
                })
            }
            Ok(_) => {
                text::search(&index, &schema, query_str, filters, limit).unwrap_or_else(|e| {
                    eprintln!("Search error: {e}");
                    std::process::exit(1);
                })
            }
            Err(_) => text::search(&index, &schema, query_str, filters, limit).unwrap_or_else(
                |e| {
                    eprintln!("Search error: {e}");
                    std::process::exit(1);
                },
            ),
        },
    };

    if json_output {
        format::print_search_results_json(&results);
    } else if let Some(ctx_n) = context {
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
    use tantivy::schema::Value as TantivyValue;
    let message_id_field = schema.get_field("message_id").ok()?;
    let term = tantivy::Term::from_field_text(message_id_field, message_id);
    let q = tantivy::query::TermQuery::new(term, IndexRecordOption::Basic);
    let top = searcher
        .search(&q, &tantivy::collector::TopDocs::with_limit(1))
        .ok()?;
    let (_, doc_addr) = top.into_iter().next()?;
    let doc: tantivy::TantivyDocument = searcher.doc(doc_addr).ok()?;

    fn get(
        doc: &tantivy::TantivyDocument,
        schema: &tantivy::schema::Schema,
        name: &str,
    ) -> String {
        let field = schema.get_field(name).unwrap();
        doc.get_first(field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    Some(query::text::SearchResult {
        session_id: get(&doc, schema, "session_id"),
        message_id: get(&doc, schema, "message_id"),
        project: get(&doc, schema, "project"),
        role: get(&doc, schema, "role"),
        content_type: get(&doc, schema, "content_type"),
        tool_name: get(&doc, schema, "tool_name"),
        file_path: get(&doc, schema, "file_path"),
        content: get(&doc, schema, "content"),
        score: 0.0,
        sequence: 0,
    })
}

/// Resolve a `--project` hint (substring) to the set of project directory
/// names that match. Returns empty if no hint; returns empty + caller should
/// error if hint given but no match.
fn resolve_project_dirs(all_sessions: &[SessionFile], hint: Option<&str>) -> Vec<String> {
    let Some(hint) = hint else { return Vec::new() };
    let mut out: Vec<String> = all_sessions
        .iter()
        .map(|s| s.project.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|p| p.contains(hint))
        .collect();
    out.sort();
    out.dedup();
    out
}

fn cmd_sessions(config: &Config, project_filter: Option<&str>, after: Option<&str>, sort: &str) {
    let state = IndexState::load(&config.state_file);
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);
    let meta_map = metadata::load_all_session_meta(&config.claude_session_meta_dir);

    let after_dt = after.and_then(parse_date);

    let mut items: Vec<SessionListItem> = all_sessions
        .into_iter()
        .filter(|s| match project_filter {
            Some(pf) => s.project.contains(pf),
            None => true,
        })
        .filter_map(|s| {
            let derived = state.indexed_sessions.get(&s.session_id).and_then(|e| e.meta.clone());
            let legacy_meta = meta_map.get(&s.session_id).cloned();

            let start_time = derived
                .as_ref()
                .and_then(|d| d.start_time)
                .or_else(|| legacy_meta.as_ref().and_then(|m| m.start_time))
                .or_else(|| {
                    s.modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|d| chrono::DateTime::from_timestamp(d.as_secs() as i64, 0))
                });

            if let Some(after_dt) = after_dt {
                if let Some(st) = start_time {
                    if st < after_dt {
                        return None;
                    }
                }
            }

            let first_prompt = derived
                .as_ref()
                .and_then(|d| d.first_prompt.clone())
                .or_else(|| legacy_meta.as_ref().and_then(|m| m.first_prompt.clone()));

            Some(SessionListItem {
                session_id: s.session_id,
                project: s.project,
                start_time: start_time.map(|t| t.format("%Y-%m-%d %H:%M").to_string()),
                first_prompt,
                duration_minutes: derived
                    .as_ref()
                    .and_then(|d| d.duration_minutes())
                    .or_else(|| legacy_meta.as_ref().and_then(|m| m.duration_minutes)),
                input_tokens: derived
                    .as_ref()
                    .map(|d| d.input_tokens)
                    .or_else(|| legacy_meta.as_ref().and_then(|m| m.input_tokens)),
                output_tokens: derived
                    .as_ref()
                    .map(|d| d.output_tokens)
                    .or_else(|| legacy_meta.as_ref().and_then(|m| m.output_tokens)),
                indexed: derived.is_some(),
            })
        })
        .collect();

    match sort {
        "tokens" => {
            items.sort_by(|a, b| {
                let ta = a.input_tokens.unwrap_or(0);
                let tb = b.input_tokens.unwrap_or(0);
                tb.cmp(&ta)
            });
        }
        "duration" => {
            items.sort_by(|a, b| {
                let da = a.duration_minutes.unwrap_or(0.0);
                let db = b.duration_minutes.unwrap_or(0.0);
                db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        _ => {
            items.sort_by(|a, b| {
                let ta = a.start_time.as_deref().unwrap_or("");
                let tb = b.start_time.as_deref().unwrap_or("");
                tb.cmp(ta)
            });
        }
    }

    format::print_session_list(&items);
}

fn cmd_projects(config: &Config) {
    let state = IndexState::load(&config.state_file);
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);

    let mut counts: HashMap<String, (usize, usize)> = HashMap::new(); // (total, indexed)
    for s in &all_sessions {
        let entry = counts.entry(s.project.clone()).or_insert((0, 0));
        entry.0 += 1;
        if state.indexed_sessions.contains_key(&s.session_id) {
            entry.1 += 1;
        }
    }

    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));

    println!("{} project(s)\n", sorted.len());
    println!("{:<60}  {:>8}  {:>8}", "project", "sessions", "indexed");
    for (project, (total, indexed)) in sorted {
        println!("{:<60}  {:>8}  {:>8}", project, total, indexed);
    }
}

fn cmd_show(
    config: &Config,
    session_id_prefix: &str,
    filter: ShowFilter,
    file_scope: Option<&str>,
    json_output: bool,
) {
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);

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
            let filtered: Vec<_> = records
                .iter()
                .filter(|r| match file_scope {
                    Some(needle) => r
                        .file_path
                        .as_deref()
                        .map(|fp| fp.contains(needle))
                        .unwrap_or(false),
                    None => true,
                })
                .cloned()
                .collect();
            if json_output {
                format::print_session_show_json(&filtered, filter);
            } else {
                format::print_session_show(&filtered, filter);
            }
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

fn cmd_recover(
    config: &Config,
    file_query: &str,
    session_prefix: Option<&str>,
    project_filter: Option<&str>,
    tool_filter: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
    format: &str,
) {
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);

    let sessions: Vec<_> = all_sessions
        .iter()
        .filter(|s| match project_filter {
            Some(pf) => s.project.contains(pf),
            None => true,
        })
        .filter(|s| match session_prefix {
            Some(sp) => s.session_id.starts_with(sp),
            None => true,
        })
        .collect();

    let tools: Vec<String> = match tool_filter {
        Some(t) => vec![t.to_string()],
        None => vec!["Edit".to_string(), "Write".to_string()],
    };

    let filter = parse::tool_events::EventFilter {
        tools,
        file_substr: Some(file_query.to_string()),
        since: since.and_then(parse_date),
        until: until.and_then(parse_date),
    };

    let mut all_events: Vec<parse::tool_events::ToolUseEvent> = Vec::new();
    for s in &sessions {
        let events = parse::tool_events::extract_tool_uses(s, &filter);
        all_events.extend(events);
    }

    all_events.sort_by_key(|e| e.timestamp.unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC));

    match format {
        "human" => {
            if all_events.is_empty() {
                println!("No matching tool events.");
                return;
            }
            println!("{} event(s)\n", all_events.len());
            for e in &all_events {
                let ts = e
                    .timestamp
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                println!(
                    "{}  {}  {}  {}",
                    ts,
                    e.tool,
                    e.session_id,
                    e.file_path.as_deref().unwrap_or("?"),
                );
                if let Some(old) = e.input.get("old_string").and_then(|v| v.as_str()) {
                    println!("--- old ({}B) ---\n{}", old.len(), old);
                }
                if let Some(new) = e.input.get("new_string").and_then(|v| v.as_str()) {
                    println!("--- new ({}B) ---\n{}", new.len(), new);
                }
                if let Some(content) = e.input.get("content").and_then(|v| v.as_str()) {
                    println!("--- content ({}B) ---\n{}", content.len(), content);
                }
                println!();
            }
        }
        _ => {
            // JSON — default
            println!(
                "{}",
                serde_json::to_string_pretty(&all_events)
                    .expect("serialize tool events")
            );
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
    let state = IndexState::load(&config.state_file);
    let all_sessions = session::discover_sessions(&config.claude_projects_dir);
    let legacy_meta = metadata::load_all_session_meta(&config.claude_session_meta_dir);

    let sessions: Vec<_> = all_sessions
        .into_iter()
        .filter(|s| match project_filter {
            Some(pf) => s.project.contains(pf),
            None => true,
        })
        .collect();

    let mut total_input_tokens = 0u64;
    let mut total_output_tokens = 0u64;
    let mut total_duration = 0.0f64;
    let mut total_messages = 0u64;
    let mut tool_counts: HashMap<String, u64> = HashMap::new();
    let mut files_touched: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut projects: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut earliest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut latest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut unindexed = 0usize;

    for sf in &sessions {
        projects.insert(sf.project.clone());
        let derived = state.indexed_sessions.get(&sf.session_id).and_then(|e| e.meta.as_ref());

        if let Some(d) = derived {
            total_input_tokens += d.input_tokens;
            total_output_tokens += d.output_tokens;
            if let Some(dur) = d.duration_minutes() {
                total_duration += dur;
            }
            total_messages += d.user_message_count + d.assistant_message_count;
            for (tool, count) in &d.tool_counts {
                *tool_counts.entry(tool.clone()).or_default() += count;
            }
            for f in &d.files_modified {
                files_touched.insert(f.clone());
            }
            if let Some(st) = d.start_time {
                earliest = Some(earliest.map_or(st, |e| e.min(st)));
                latest = Some(latest.map_or(st, |l| l.max(st)));
            }
        } else if let Some(m) = legacy_meta.get(&sf.session_id) {
            total_input_tokens += m.input_tokens.unwrap_or(0);
            total_output_tokens += m.output_tokens.unwrap_or(0);
            total_duration += m.duration_minutes.unwrap_or(0.0);
            total_messages +=
                m.user_message_count.unwrap_or(0) + m.assistant_message_count.unwrap_or(0);
            if let Some(ref tc) = m.tool_counts {
                for (tool, count) in tc {
                    *tool_counts.entry(tool.clone()).or_default() += count;
                }
            }
            if let Some(st) = m.start_time {
                earliest = Some(earliest.map_or(st, |e| e.min(st)));
                latest = Some(latest.map_or(st, |l| l.max(st)));
            }
        } else {
            unindexed += 1;
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
        unindexed_sessions: unindexed,
    };

    if json_output {
        format::print_stats_json(&stats);
    } else {
        format::print_stats(&stats);
    }
}

fn cmd_service(action: ServiceAction) {
    match action {
        ServiceAction::Install { interval } => {
            let iv = match service::parse_interval(&interval) {
                Ok(iv) => iv,
                Err(e) => {
                    eprintln!("Bad --interval: {e}");
                    std::process::exit(1);
                }
            };
            match service::install(&iv) {
                Ok(msg) => println!("{msg}"),
                Err(e) => {
                    eprintln!("Install failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        ServiceAction::Uninstall => match service::uninstall() {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("Uninstall failed: {e}");
                std::process::exit(1);
            }
        },
        ServiceAction::Status => match service::status() {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("Status failed: {e}");
                std::process::exit(1);
            }
        },
    }
}

fn cmd_watch(config: &Config, project_filter: Option<&str>, no_embed: bool) {
    use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
    use std::sync::mpsc;
    use std::time::Duration;

    if let Err(e) = config.ensure_dirs() {
        eprintln!("Error creating data directories: {e}");
        std::process::exit(1);
    }

    eprintln!("watching {}", config.claude_projects_dir.display());
    eprintln!("running initial incremental index...");
    cmd_index(config, false, project_filter, false, no_embed);

    let (tx, rx) = mpsc::channel();
    let mut debouncer = match new_debouncer(Duration::from_secs(2), tx) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("watch error: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = debouncer
        .watcher()
        .watch(&config.claude_projects_dir, RecursiveMode::Recursive)
    {
        eprintln!("watch error: {e}");
        std::process::exit(1);
    }

    eprintln!("idle — watching for changes (Ctrl-C to exit).");

    loop {
        match rx.recv() {
            Ok(Ok(events)) => {
                let touched = events
                    .iter()
                    .any(|e| e.path.extension().is_some_and(|x| x == "jsonl"));
                if touched {
                    eprintln!("detected change; reindexing...");
                    cmd_index(config, false, project_filter, false, no_embed);
                    eprintln!("idle.");
                }
            }
            Ok(Err(err)) => {
                eprintln!("watch notify error: {err}");
            }
            Err(e) => {
                eprintln!("watch channel closed: {e}");
                return;
            }
        }
    }
}
