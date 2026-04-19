use tantivy::collector::TopDocs;
use tantivy::query::{AllQuery, BooleanQuery, Occur, QueryParser, RangeQuery, TermQuery};
use tantivy::schema::*;
use tantivy::{Index, TantivyDocument};

use super::filters::SearchFilters;

/// A single search result.
#[derive(Debug)]
pub struct SearchResult {
    pub session_id: String,
    pub message_id: String,
    pub project: String,
    pub role: String,
    pub content_type: String,
    pub tool_name: String,
    pub file_path: String,
    pub content: String,
    pub score: f32,
    pub sequence: u64,
}

/// Run a full-text search with optional filters. `query_str` may be empty when
/// filters alone are enough (e.g. `--tool Edit --file foo.rs`).
pub fn search(
    index: &Index,
    schema: &Schema,
    query_str: &str,
    filters: &SearchFilters,
    limit: usize,
) -> tantivy::Result<Vec<SearchResult>> {
    let reader = index.reader()?;
    let searcher = reader.searcher();

    let content_field = schema.get_field("content").unwrap();
    let file_path_field = schema.get_field("file_path").unwrap();
    let command_field = schema.get_field("command").unwrap();

    let mut clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();

    let trimmed = query_str.trim();
    if trimmed.is_empty() {
        clauses.push((Occur::Must, Box::new(AllQuery)));
    } else {
        let query_parser =
            QueryParser::for_index(index, vec![content_field, file_path_field, command_field]);
        let text_query = query_parser.parse_query(trimmed)?;
        clauses.push((Occur::Must, text_query));
    }

    if let Some(ref role) = filters.role {
        let field = schema.get_field("role").unwrap();
        let term = tantivy::Term::from_field_text(field, role);
        clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
    }

    if let Some(ref tool) = filters.tool {
        let field = schema.get_field("tool_name").unwrap();
        let term = tantivy::Term::from_field_text(field, tool);
        clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
    }

    if !filters.project_dirs.is_empty() {
        let field = schema.get_field("project").unwrap();
        if filters.project_dirs.len() == 1 {
            let term = tantivy::Term::from_field_text(field, &filters.project_dirs[0]);
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
            ));
        } else {
            let mut project_clauses: Vec<(Occur, Box<dyn tantivy::query::Query>)> = Vec::new();
            for dir in &filters.project_dirs {
                let term = tantivy::Term::from_field_text(field, dir);
                project_clauses.push((
                    Occur::Should,
                    Box::new(TermQuery::new(term, IndexRecordOption::Basic)),
                ));
            }
            clauses.push((Occur::Must, Box::new(BooleanQuery::new(project_clauses))));
        }
    }

    if let Some(ref ct) = filters.content_type {
        let field = schema.get_field("content_type").unwrap();
        let term = tantivy::Term::from_field_text(field, ct);
        clauses.push((Occur::Must, Box::new(TermQuery::new(term, IndexRecordOption::Basic))));
    }

    if let Some(ref file_substr) = filters.file_path {
        // file_path is a TEXT field (tokenized), so a QueryParser phrase handles substrings
        // of path components. Accept both literal and tokenized matches.
        let parser = QueryParser::for_index(index, vec![file_path_field]);
        let quoted = format!("\"{}\"", file_substr.replace('"', ""));
        if let Ok(q) = parser.parse_query(&quoted) {
            clauses.push((Occur::Must, q));
        } else if let Ok(q) = parser.parse_query(file_substr) {
            clauses.push((Occur::Must, q));
        }
    }

    let far_past = tantivy::DateTime::from_timestamp_secs(0);
    let far_future = tantivy::DateTime::from_timestamp_secs(4102444800);

    if filters.after.is_some() || filters.before.is_some() {
        let start = filters
            .after
            .map(|a| tantivy::DateTime::from_timestamp_secs(a.timestamp()))
            .unwrap_or(far_past);
        let end = filters
            .before
            .map(|b| tantivy::DateTime::from_timestamp_secs(b.timestamp()))
            .unwrap_or(far_future);
        clauses.push((
            Occur::Must,
            Box::new(RangeQuery::new_date("timestamp".to_string(), start..end)),
        ));
    }

    let combined = BooleanQuery::new(clauses);
    let top_docs = searcher.search(&combined, &TopDocs::with_limit(limit))?;

    let mut results = Vec::new();
    for (score, doc_address) in top_docs {
        let doc: TantivyDocument = searcher.doc(doc_address)?;
        results.push(SearchResult {
            session_id: get_text(&doc, schema, "session_id"),
            message_id: get_text(&doc, schema, "message_id"),
            project: get_text(&doc, schema, "project"),
            role: get_text(&doc, schema, "role"),
            content_type: get_text(&doc, schema, "content_type"),
            tool_name: get_text(&doc, schema, "tool_name"),
            file_path: get_text(&doc, schema, "file_path"),
            content: get_text(&doc, schema, "content"),
            score,
            sequence: get_u64(&doc, schema, "sequence"),
        });
    }

    Ok(results)
}

fn get_text(doc: &TantivyDocument, schema: &Schema, field_name: &str) -> String {
    let field = schema.get_field(field_name).unwrap();
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn get_u64(doc: &TantivyDocument, schema: &Schema, field_name: &str) -> u64 {
    let field = schema.get_field(field_name).unwrap();
    doc.get_first(field)
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}
