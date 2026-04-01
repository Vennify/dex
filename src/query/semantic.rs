use crate::embed::model::Embedder;
use crate::index::vector::VectorStore;

/// A semantic search result.
#[derive(Debug)]
pub struct SemanticResult {
    pub session_id: String,
    pub message_id: String,
    pub distance: f32,
    pub rank: usize,
}

/// Run a semantic search: embed the query, then ANN search.
pub fn search(
    embedder: &mut Embedder,
    vector_store: &VectorStore,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticResult>, String> {
    let query_embedding = embedder.embed(query)?;

    let results = vector_store.search(&query_embedding, limit)?;

    Ok(results
        .into_iter()
        .enumerate()
        .map(|(rank, (_key, distance, meta))| SemanticResult {
            session_id: meta.session_id,
            message_id: meta.message_id,
            distance,
            rank,
        })
        .collect())
}
