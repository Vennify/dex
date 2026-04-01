use tokenizers::Tokenizer;

const MAX_CHUNK_TOKENS: usize = 512;
const OVERLAP_TOKENS: usize = 64;

/// A chunk of text with its character offsets.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub text: String,
    pub start_char: usize,
    pub end_char: usize,
    pub index: usize,
}

/// Split text into chunks of at most MAX_CHUNK_TOKENS tokens with OVERLAP_TOKENS overlap.
/// Returns the text chunks with their character offsets.
pub fn chunk_text(text: &str, tokenizer: &Tokenizer) -> Vec<Chunk> {
    let encoding = match tokenizer.encode(text, false) {
        Ok(e) => e,
        Err(_) => return vec![single_chunk(text)],
    };

    let token_count = encoding.get_ids().len();
    if token_count <= MAX_CHUNK_TOKENS {
        return vec![single_chunk(text)];
    }

    let offsets = encoding.get_offsets();
    let mut chunks = Vec::new();
    let mut start_token = 0;
    let mut chunk_index = 0;

    while start_token < token_count {
        let end_token = (start_token + MAX_CHUNK_TOKENS).min(token_count);

        let start_char = offsets[start_token].0;
        let end_char = offsets[end_token - 1].1;

        let chunk_text = &text[start_char..end_char];
        chunks.push(Chunk {
            text: chunk_text.to_string(),
            start_char,
            end_char,
            index: chunk_index,
        });

        chunk_index += 1;

        if end_token >= token_count {
            break;
        }

        // Advance with overlap
        let step = MAX_CHUNK_TOKENS - OVERLAP_TOKENS;
        start_token += step;
    }

    chunks
}

fn single_chunk(text: &str) -> Chunk {
    Chunk {
        text: text.to_string(),
        start_char: 0,
        end_char: text.len(),
        index: 0,
    }
}
