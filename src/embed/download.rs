use std::path::{Path, PathBuf};
use std::io::Write;

use indicatif::{ProgressBar, ProgressStyle};

const MODEL_REPO: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";
const ONNX_FILE: &str = "onnx/model.onnx";
const TOKENIZER_FILE: &str = "tokenizer.json";

/// Ensure the ONNX model and tokenizer are present. Downloads on first run.
/// Returns (model_path, tokenizer_path).
pub fn ensure_model(models_dir: &Path) -> Result<(PathBuf, PathBuf), String> {
    std::fs::create_dir_all(models_dir).map_err(|e| format!("failed to create models dir: {e}"))?;

    let model_path = models_dir.join("all-MiniLM-L6-v2.onnx");
    let tokenizer_path = models_dir.join("tokenizer.json");

    if !model_path.exists() {
        let url = format!("{MODEL_REPO}/{ONNX_FILE}");
        download_file(&url, &model_path)?;
    }

    if !tokenizer_path.exists() {
        let url = format!("{MODEL_REPO}/{TOKENIZER_FILE}");
        download_file(&url, &tokenizer_path)?;
    }

    Ok((model_path, tokenizer_path))
}

fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    let filename = dest.file_name().unwrap_or_default().to_string_lossy();
    eprintln!("Downloading {filename}...");

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let resp = client
        .get(url)
        .send()
        .map_err(|e| format!("download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }

    let total_size = resp.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("=>-"),
    );

    let bytes = resp.bytes().map_err(|e| format!("download read error: {e}"))?;
    pb.set_position(bytes.len() as u64);

    let tmp_path = dest.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp_path)
        .map_err(|e| format!("failed to create {}: {e}", tmp_path.display()))?;
    file.write_all(&bytes)
        .map_err(|e| format!("failed to write {}: {e}", tmp_path.display()))?;

    std::fs::rename(&tmp_path, dest)
        .map_err(|e| format!("failed to rename tmp to {}: {e}", dest.display()))?;

    pb.finish_and_clear();
    eprintln!("Downloaded {filename} ({} bytes)", bytes.len());
    Ok(())
}
