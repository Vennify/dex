use std::path::Path;

use serde::{Deserialize, Serialize};
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};
use usearch::Index;

use crate::embed::model::EMBEDDING_DIM;

/// Metadata for a single vector in the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorMeta {
    pub session_id: String,
    pub message_id: String,
    pub chunk_index: usize,
}

/// Vector store wrapping USearch HNSW index + metadata sidecar.
pub struct VectorStore {
    index: Index,
    meta: Vec<VectorMeta>,
    next_key: u64,
    index_path: std::path::PathBuf,
    meta_path: std::path::PathBuf,
}

impl VectorStore {
    /// Open or create a vector store.
    pub fn open(data_dir: &Path) -> Result<Self, String> {
        let index_path = data_dir.join("vectors.usearch");
        let meta_path = data_dir.join("vectors_meta.json");

        let options = IndexOptions {
            dimensions: EMBEDDING_DIM,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            connectivity: 16,
            expansion_add: 128,
            expansion_search: 64,
            multi: false,
        };

        let index = Index::new(&options).map_err(|e| format!("usearch index create error: {e}"))?;

        // Load existing index if present
        if index_path.exists() {
            index
                .load(index_path.to_str().unwrap())
                .map_err(|e| format!("usearch load error: {e}"))?;
        } else {
            // Reserve initial capacity
            index
                .reserve(100_000)
                .map_err(|e| format!("usearch reserve error: {e}"))?;
        }

        // Load metadata sidecar
        let meta: Vec<VectorMeta> = if meta_path.exists() {
            let data = std::fs::read_to_string(&meta_path)
                .map_err(|e| format!("meta read error: {e}"))?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Vec::new()
        };

        let next_key = index.size() as u64;

        Ok(VectorStore {
            index,
            meta,
            next_key,
            index_path,
            meta_path,
        })
    }

    /// Add a vector with metadata. Returns the assigned key.
    pub fn add(&mut self, embedding: &[f32], meta: VectorMeta) -> Result<u64, String> {
        let key = self.next_key;

        // Ensure capacity
        let current_cap = self.index.capacity();
        if key as usize >= current_cap {
            self.index
                .reserve(current_cap + 100_000)
                .map_err(|e| format!("usearch reserve error: {e}"))?;
        }

        self.index
            .add(key, embedding)
            .map_err(|e| format!("usearch add error: {e}"))?;

        // Ensure meta vec is long enough
        while self.meta.len() <= key as usize {
            self.meta.push(VectorMeta {
                session_id: String::new(),
                message_id: String::new(),
                chunk_index: 0,
            });
        }
        self.meta[key as usize] = meta;

        self.next_key = key + 1;
        Ok(key)
    }

    /// Search for the k nearest neighbors. Returns Vec of (key, distance, meta).
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32, VectorMeta)>, String> {
        let results = self.index
            .search(query, k)
            .map_err(|e| format!("usearch search error: {e}"))?;

        let mut out = Vec::new();
        for i in 0..results.keys.len() {
            let key = results.keys[i];
            let distance = results.distances[i];
            let meta = if (key as usize) < self.meta.len() {
                self.meta[key as usize].clone()
            } else {
                continue;
            };
            out.push((key, distance, meta));
        }

        Ok(out)
    }

    /// Save index and metadata to disk.
    pub fn save(&self) -> Result<(), String> {
        self.index
            .save(self.index_path.to_str().unwrap())
            .map_err(|e| format!("usearch save error: {e}"))?;

        let meta_json = serde_json::to_string(&self.meta)
            .map_err(|e| format!("meta serialize error: {e}"))?;
        std::fs::write(&self.meta_path, meta_json)
            .map_err(|e| format!("meta write error: {e}"))?;

        Ok(())
    }

    /// Number of vectors in the index.
    pub fn len(&self) -> usize {
        self.index.size()
    }

    /// Remove all vectors for a given session (by scanning metadata).
    /// Note: USearch doesn't support efficient deletion, so we just mark them.
    /// For full correctness on re-index, we'd rebuild. For incremental this is fine
    /// since session IDs don't repeat.
    pub fn remove_session(&mut self, session_id: &str) {
        for (key, meta) in self.meta.iter().enumerate() {
            if meta.session_id == session_id {
                let _ = self.index.remove(key as u64);
            }
        }
    }
}
