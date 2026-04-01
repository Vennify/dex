# dex — Claude Code Conversation Indexer

Local-first CLI tool for indexing, searching, and querying Claude Code conversation history. Combines Tantivy full-text search with local semantic embeddings for hybrid retrieval across thousands of sessions.

## Problem

Claude Code stores conversations as JSONL files in `~/.claude/projects/`. Power users accumulate hundreds of sessions totaling gigabytes. Finding "that conversation where I debugged X" requires manually opening files and scanning. There's no search, no filtering, no way to query by tool type or file touched.

## Data Source

All data lives under `~/.claude/`:

```
~/.claude/
├── projects/
│   └── <project-path>/          # e.g. -home-user-myproject
│       ├── <uuid>.jsonl         # conversation messages (one per session)
│       └── <uuid>/              # session artifacts (file snapshots, etc.)
├── usage-data/
│   └── session-meta/
│       └── <uuid>.json          # session metadata (timestamps, token counts, first prompt)
└── history.jsonl                # global prompt history (user messages with project + timestamp)
```

### JSONL Message Types

Each line in a session JSONL is one of:

| `type` | Description |
|--------|-------------|
| `user` | User message. `message.role = "user"`, `message.content` is string or content blocks |
| `assistant` | Assistant response. `message.content` is array of content blocks |
| `system` | System/context injection |
| `progress` | Streaming progress for tool calls (ignorable for indexing) |
| `file-history-snapshot` | File state snapshots (ignorable for indexing) |

### Content Block Types (within assistant messages)

| `type` | Description |
|--------|-------------|
| `text` | Assistant text response |
| `thinking` | Chain-of-thought (may want to index for semantic search, exclude from display) |
| `tool_use` | Tool invocation: `{name, id, input}` |
| `tool_result` | Tool output: `{tool_use_id, content}` |

### Tool Call Structure

```json
{
  "type": "tool_use",
  "id": "toolu_xxx",
  "name": "Edit",
  "input": {
    "file_path": "/home/user/project/src/main.rs",
    "old_string": "...",
    "new_string": "..."
  }
}
```

Key tools to extract structured data from: `Edit` (file_path, old/new), `Read` (file_path), `Write` (file_path), `Bash` (command), `Grep` (pattern, path), `Glob` (pattern), `Agent` (prompt, subagent_type).

### Session Metadata

```json
{
  "session_id": "uuid",
  "project_path": "/home/user/project",
  "start_time": "2026-03-14T17:22:25.325Z",
  "duration_minutes": 45,
  "user_message_count": 12,
  "assistant_message_count": 10,
  "tool_counts": {"Edit": 5, "Bash": 3},
  "input_tokens": 50000,
  "output_tokens": 30000,
  "first_prompt": "fix the websocket reconnection bug",
  "lines_added": 150,
  "lines_removed": 30,
  "files_modified": 4
}
```

## Architecture

```
┌─────────┐     ┌──────────┐     ┌───────────┐
│  JSONL   │────▶│  Parser  │────▶│  Records  │
│  files   │     └──────────┘     └─────┬─────┘
└─────────┘                             │
                              ┌─────────┴─────────┐
                              ▼                     ▼
                     ┌──────────────┐      ┌──────────────┐
                     │   Tantivy    │      │   Embedder   │
                     │  full-text   │      │  (ort/ONNX)  │
                     │  + struct    │      └──────┬───────┘
                     └──────────────┘             ▼
                                          ┌──────────────┐
                                          │   USearch     │
                                          │  vector ANN   │
                                          └──────────────┘
                              ┌─────────┬─────────┐
                              ▼                     ▼
                     ┌──────────────┐      ┌──────────────┐
                     │  Text Query  │      │ Semantic Query│
                     └──────┬───────┘      └──────┬───────┘
                            └─────────┬───────────┘
                                      ▼
                              ┌──────────────┐
                              │  RRF Merger  │
                              │  (hybrid)    │
                              └──────────────┘
```

### Crate Layout

```
dex/
├── Cargo.toml
├── src/
│   ├── main.rs              # CLI entry point (clap)
│   ├── config.rs            # paths, settings, defaults
│   ├── parse/
│   │   ├── mod.rs
│   │   ├── session.rs       # JSONL parser → Record stream
│   │   ├── metadata.rs      # session-meta JSON parser
│   │   ├── content.rs       # content block extraction (text, tool_use, etc.)
│   │   └── tools.rs         # tool-specific field extraction (file paths, commands)
│   ├── index/
│   │   ├── mod.rs
│   │   ├── tantivy.rs       # tantivy schema, indexing, text queries
│   │   ├── vector.rs        # usearch index, ANN queries
│   │   └── state.rs         # incremental index state (what's been indexed)
│   ├── embed/
│   │   ├── mod.rs
│   │   ├── model.rs         # ONNX model loading + inference
│   │   ├── chunker.rs       # text chunking with overlap
│   │   └── download.rs      # model auto-download on first run
│   ├── query/
│   │   ├── mod.rs
│   │   ├── text.rs          # tantivy query builder (with filters)
│   │   ├── semantic.rs      # embed query → ANN search
│   │   ├── hybrid.rs        # reciprocal rank fusion
│   │   └── filters.rs       # role, tool, file, project, date range
│   └── output/
│       ├── mod.rs
│       └── format.rs        # terminal output formatting
├── models/                  # gitignored, populated on first run
└── tests/
    ├── fixtures/            # sample JSONL files
    └── integration.rs
```

## Tantivy Schema

```rust
struct Document {
    // Identity
    session_id:   String,    // stored, indexed
    message_id:   String,    // stored, indexed (unique per content block)

    // Taxonomy
    project:      String,    // stored, faceted — project path
    role:         String,    // stored, faceted — user | assistant | system
    content_type: String,    // stored, faceted — text | tool_use | tool_result | thinking
    tool_name:    String,    // stored, faceted — Edit | Bash | Grep | Read | Write | Agent | ...

    // Extracted fields
    file_path:    String,    // stored, indexed — extracted from tool inputs
    command:      String,    // stored, indexed — extracted from Bash tool inputs

    // Content
    content:      Text,      // stored, full-text indexed — the searchable text

    // Ordering
    timestamp:    DateTime,  // stored, fast-field — message timestamp
    sequence:     u64,       // stored, fast-field — position within session

    // Vector link
    chunk_ids:    Vec<u64>,  // stored — pointers into usearch index
}
```

### What gets indexed as `content`

| Source | Indexed text |
|--------|-------------|
| User message | Raw message text (strip system-reminder tags) |
| Assistant text block | Raw text |
| Assistant thinking block | Raw thinking text (flagged as `content_type=thinking`) |
| Tool use (Edit) | `"Edit {file_path}: {summary of change}"` — synthesized from old/new strings |
| Tool use (Bash) | `"Bash: {command}"` |
| Tool use (Grep) | `"Grep {pattern} in {path}"` |
| Tool use (Read) | `"Read {file_path}"` |
| Tool use (Write) | `"Write {file_path}"` |
| Tool use (Agent) | `"Agent ({subagent_type}): {prompt}"` |
| Tool result | Indexed but with lower boost weight. Large results (>2000 chars) truncated. |

## Embedding Pipeline

### Model

`all-MiniLM-L6-v2` — 384-dimensional vectors, ~80MB ONNX file.

- Loaded via `ort` (ONNX Runtime Rust bindings)
- Tokenizer via `tokenizers` (HuggingFace)
- Auto-downloaded to `~/.local/share/dex/models/` on first `dex index`
- No Python, no server, no GPU required

### What gets embedded

- User messages (full text)
- Assistant text blocks (chunked if >512 tokens)
- Tool use blocks (synthesized text, same as tantivy `content`)
- Thinking blocks (chunked if >512 tokens)
- NOT tool results (too noisy — output of grep/read/bash is not semantically useful for retrieval)

### Chunking

- Max chunk size: 512 tokens (model context is 512)
- Overlap: 64 tokens between chunks
- Each chunk gets its own vector in usearch
- Chunk metadata: `(session_id, message_id, chunk_index, start_char, end_char)`

### Vector Store

USearch with HNSW index:
- Metric: cosine similarity
- Stored at `~/.local/share/dex/vectors.usearch`
- Separate metadata sidecar (`vectors_meta.bin`) mapping vector ID → (session_id, message_id, chunk_index)

## CLI Interface

### Indexing

```bash
dex index                          # incremental index (only new/changed sessions)
dex index --full                   # full reindex from scratch
dex index --project ~/myproject    # index only one project
dex index --status                 # show index stats (sessions, documents, vectors, size)
```

### Search

```bash
# Hybrid search (default) — combines full-text + semantic, RRF ranking
dex search "websocket reconnection debugging"

# Exact full-text only
dex search --exact "reconnect_with_backoff"

# Semantic only
dex search --semantic "that time I was fixing the auth flow"

# With filters
dex search "reconnect" --role user
dex search "reconnect" --role assistant
dex search "reconnect" --project wigwam
dex search "reconnect" --after 2026-03-01 --before 2026-03-15
dex search --tool Edit --file "relay_client.rs"
dex search --tool Bash --exact "docker compose"
dex search --tool Agent --semantic "exploring the codebase for auth"
dex search --type thinking "websocket"    # search thinking blocks only

# Output control
dex search "reconnect" --limit 20         # max results (default 10)
dex search "reconnect" --context 3        # show N surrounding messages
dex search "reconnect" --json             # JSON output for piping
```

### Sessions

```bash
dex sessions                               # list all sessions (most recent first)
dex sessions --project wigwam              # filter by project
dex sessions --after 2026-03-01            # filter by date
dex sessions --sort tokens                 # sort by token usage
dex sessions --sort duration               # sort by duration
```

### Session Inspection

```bash
dex show <session-id>                      # full conversation
dex show <session-id> --user               # only user messages
dex show <session-id> --assistant          # only assistant text
dex show <session-id> --tools              # only tool calls (name + summary)
dex show <session-id> --edits             # only Edit tool calls with diffs
dex show <session-id> --files             # list all files touched
dex show <session-id> --commands          # list all Bash commands run
```

### File History (across sessions)

```bash
dex file "src/main.rs"                     # all sessions that touched this file
dex file "src/main.rs" --edits            # all edits to this file across all sessions
dex file "src/main.rs" --reads            # all reads of this file
```

### Stats

```bash
dex stats                                  # global stats
dex stats --project wigwam                 # per-project stats
# Output: session count, message count, tokens used, tools used, files touched, date range
```

## Hybrid Search: Reciprocal Rank Fusion

When running hybrid mode (default), both Tantivy and semantic search run in parallel. Results are merged using RRF:

```
score(doc) = sum over rankings r:  1 / (k + rank_in_r)
```

Where `k = 60` (standard RRF constant). This handles the score incompatibility between Tantivy BM25 scores and cosine similarities.

Filters (role, tool, project, date) are applied pre-retrieval to both indexes.

## Index Storage

```
~/.local/share/dex/
├── tantivy/              # tantivy index directory
├── vectors.usearch       # usearch HNSW index
├── vectors_meta.bin      # vector ID → document mapping
├── state.json            # incremental index state
└── models/
    ├── all-MiniLM-L6-v2.onnx
    └── tokenizer.json
```

`state.json` tracks:
```json
{
  "indexed_sessions": {
    "uuid-1": { "size": 45230, "modified": "2026-03-14T17:22:25Z" },
    "uuid-2": { "size": 12000, "modified": "2026-03-15T10:00:00Z" }
  },
  "last_full_index": "2026-03-14T20:00:00Z",
  "tantivy_doc_count": 50000,
  "vector_count": 30000
}
```

Incremental index: scan `~/.claude/projects/`, compare file sizes/mtimes against `state.json`, only parse and index changed files.

## Dependencies

```toml
[dependencies]
# Search
tantivy = "0.22"
usearch = "2"

# Embeddings
ort = "2"
tokenizers = "0.19"

# CLI
clap = { version = "4", features = ["derive"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Time
chrono = { version = "0.4", features = ["serde"] }

# UX
indicatif = "0.17"          # progress bars
colored = "2"               # terminal colors
textwrap = "0.16"           # text wrapping for display

# Utilities
directories = "5"           # XDG paths
uuid = { version = "1", features = ["v4"] }
rayon = "1"                 # parallel indexing
reqwest = { version = "0.12", features = ["blocking"] }  # model download
sha2 = "0.10"               # model integrity check
```

## Build Order

### Phase 1 — Parser + Tantivy (usable tool)

1. JSONL parser: read session files, emit normalized records
2. Tantivy schema + indexer: build full-text index
3. CLI: `index`, `search --exact`, `sessions`, `show`
4. Incremental indexing via `state.json`
5. Filters: role, tool, project, date range

At this point `dex` is usable for exact text search and structured queries.

### Phase 2 — Embeddings + Semantic Search

6. Model download + ONNX loading via `ort`
7. Text chunking pipeline
8. Embedding generation during indexing
9. USearch vector index build
10. `search --semantic` query path
11. Hybrid search with RRF (becomes the default)

### Phase 3 — Polish

12. `dex file` command (cross-session file history)
13. `dex stats` command
14. `--context N` flag (show surrounding messages)
15. `--json` output mode
16. Watch mode: `dex index --watch` (inotify on `~/.claude/projects/`)

## Performance Targets

| Operation | Target |
|-----------|--------|
| Full index (1GB JSONL) | < 5 minutes |
| Incremental index (10 new sessions) | < 5 seconds |
| Exact text search | < 50ms |
| Semantic search | < 200ms (embed query + ANN) |
| Hybrid search | < 300ms |
| Session list | < 100ms |

## Future Considerations

- **Conversation summarization**: Generate per-session summaries at index time for better semantic retrieval
- **Git integration**: Link sessions to commits made during them (timestamps + `git_commits` metadata)
- **Export**: `dex export <session-id> --markdown` for sharing conversations
- **TUI**: Interactive browser with fuzzy search (ratatui)
- **Wigwam integration**: Feed dex results into Wigwam's blame/attribution system
- **Multi-machine**: Sync indexes across machines via Wigwam relay
