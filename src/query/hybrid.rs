use std::collections::HashMap;

use tantivy::schema::{Schema, Value};
use tantivy::{Index, TantivyDocument};

use crate::embed::model::Embedder;
use crate::index::vector::VectorStore;
use crate::query::filters::SearchFilters;
use crate::query::text::{self, SearchResult};

const RRF_K: f64 = 60.0;

/// Run hybrid search: full-text + semantic, merged via Reciprocal Rank Fusion.
pub fn search(
    index: &Index,
    schema: &Schema,
    embedder: &mut Embedder,
    vector_store: &VectorStore,
    query_str: &str,
    filters: &SearchFilters,
    limit: usize,
) -> Result<Vec<SearchResult>, String> {
    // Run both searches with a wider limit to get good fusion
    let fetch_limit = limit * 5;

    let text_results = text::search(index, schema, query_str, filters, fetch_limit)
        .map_err(|e| format!("text search error: {e}"))?;

    let semantic_results = super::semantic::search(embedder, vector_store, query_str, fetch_limit)?;

    // Build RRF scores keyed by message_id
    let mut rrf_scores: HashMap<String, f64> = HashMap::new();
    let mut result_map: HashMap<String, SearchResult> = HashMap::new();

    // Text ranking
    for (rank, result) in text_results.into_iter().enumerate() {
        let score = 1.0 / (RRF_K + rank as f64 + 1.0);
        *rrf_scores.entry(result.message_id.clone()).or_default() += score;
        result_map.entry(result.message_id.clone()).or_insert(result);
    }

    // Semantic ranking — need to look up full doc info from tantivy
    let reader = index.reader().map_err(|e| format!("reader error: {e}"))?;
    let searcher = reader.searcher();

    for (rank, sem_result) in semantic_results.into_iter().enumerate() {
        let score = 1.0 / (RRF_K + rank as f64 + 1.0);
        *rrf_scores.entry(sem_result.message_id.clone()).or_default() += score;

        // If we don't already have this result from text search, look it up
        if !result_map.contains_key(&sem_result.message_id) {
            if let Some(result) = lookup_by_message_id(index, schema, &searcher, &sem_result.message_id) {
                result_map.insert(sem_result.message_id.clone(), result);
            }
        }
    }

    // Sort by RRF score descending
    let mut scored: Vec<_> = rrf_scores.into_iter().collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let results: Vec<SearchResult> = scored
        .into_iter()
        .take(limit)
        .filter_map(|(msg_id, rrf_score)| {
            result_map.remove(&msg_id).map(|mut r| {
                r.score = rrf_score as f32;
                r
            })
        })
        .collect();

    Ok(results)
}

/// Look up a document by message_id in tantivy.
fn lookup_by_message_id(
    _index: &Index,
    schema: &Schema,
    searcher: &tantivy::Searcher,
    message_id: &str,
) -> Option<SearchResult> {
    let message_id_field = schema.get_field("message_id").ok()?;
    let term = tantivy::Term::from_field_text(message_id_field, message_id);
    let query = tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);

    let top = searcher
        .search(&query, &tantivy::collector::TopDocs::with_limit(1))
        .ok()?;

    let (_, doc_addr) = top.into_iter().next()?;
    let doc: TantivyDocument = searcher.doc(doc_addr).ok()?;

    Some(SearchResult {
        session_id: get_text(&doc, schema, "session_id"),
        message_id: get_text(&doc, schema, "message_id"),
        project: get_text(&doc, schema, "project"),
        role: get_text(&doc, schema, "role"),
        content_type: get_text(&doc, schema, "content_type"),
        tool_name: get_text(&doc, schema, "tool_name"),
        file_path: get_text(&doc, schema, "file_path"),
        content: get_text(&doc, schema, "content"),
        score: 0.0,
        sequence: get_u64(&doc, schema, "sequence"),
    })
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
