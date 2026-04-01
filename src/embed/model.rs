use std::path::Path;

use ndarray::Array2;
use ort::session::Session;
use ort::value::Value;
use tokenizers::Tokenizer;

use super::chunker::{chunk_text, Chunk};

/// Embedding dimension for all-MiniLM-L6-v2.
pub const EMBEDDING_DIM: usize = 384;

/// An embedding model that produces 384-dim vectors from text.
pub struct Embedder {
    session: Session,
    tokenizer: Tokenizer,
}

impl Embedder {
    /// Load the ONNX model and tokenizer from disk.
    pub fn load(model_path: &Path, tokenizer_path: &Path) -> Result<Self, String> {
        let session = Session::builder()
            .map_err(|e| format!("ort session builder error: {e}"))?
            .with_intra_threads(4)
            .map_err(|e| format!("ort thread config error: {e}"))?
            .commit_from_file(model_path)
            .map_err(|e| format!("ort model load error: {e}"))?;

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| format!("tokenizer load error: {e}"))?;

        Ok(Embedder { session, tokenizer })
    }

    /// Get a reference to the tokenizer (for chunking).
    pub fn tokenizer(&self) -> &Tokenizer {
        &self.tokenizer
    }

    /// Embed a single text string. Returns a 384-dim vector.
    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>, String> {
        let encoding = self.tokenizer
            .encode(text, true)
            .map_err(|e| format!("tokenize error: {e}"))?;

        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = encoding.get_attention_mask().iter().map(|&m| m as i64).collect();
        let token_type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&t| t as i64).collect();
        let seq_len = input_ids.len();

        let input_ids_array = Array2::from_shape_vec((1, seq_len), input_ids)
            .map_err(|e| format!("shape error: {e}"))?;
        let attention_mask_array = Array2::from_shape_vec((1, seq_len), attention_mask.clone())
            .map_err(|e| format!("shape error: {e}"))?;
        let token_type_ids_array = Array2::from_shape_vec((1, seq_len), token_type_ids)
            .map_err(|e| format!("shape error: {e}"))?;

        let inputs = ort::inputs![
            "input_ids" => Value::from_array(input_ids_array).map_err(|e| format!("value error: {e}"))?,
            "attention_mask" => Value::from_array(attention_mask_array.clone()).map_err(|e| format!("value error: {e}"))?,
            "token_type_ids" => Value::from_array(token_type_ids_array).map_err(|e| format!("value error: {e}"))?,
        ];

        let outputs = self.session
            .run(inputs)
            .map_err(|e| format!("inference error: {e}"))?;

        // Output shape is [1, seq_len, 384] — we need mean pooling
        let (_shape, output_data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("extract error: {e}"))?;

        // Mean pooling with attention mask
        let mut pooled = vec![0.0f32; EMBEDDING_DIM];
        let mut mask_sum = 0.0f32;

        for tok in 0..seq_len {
            let mask_val = attention_mask[tok] as f32;
            mask_sum += mask_val;
            for dim in 0..EMBEDDING_DIM {
                let idx = tok * EMBEDDING_DIM + dim;
                pooled[dim] += output_data[idx] * mask_val;
            }
        }

        if mask_sum > 0.0 {
            for dim in 0..EMBEDDING_DIM {
                pooled[dim] /= mask_sum;
            }
        }

        // L2 normalize
        let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut pooled {
                *x /= norm;
            }
        }

        Ok(pooled)
    }

    /// Embed text, chunking if necessary. Returns Vec of (chunk, embedding).
    pub fn embed_chunked(&mut self, text: &str) -> Result<Vec<(Chunk, Vec<f32>)>, String> {
        let chunks = chunk_text(text, &self.tokenizer);
        let mut results = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let embedding = self.embed(&chunk.text)?;
            results.push((chunk, embedding));
        }
        Ok(results)
    }
}
