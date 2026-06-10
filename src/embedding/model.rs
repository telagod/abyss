use std::path::PathBuf;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use tracing::info;

use crate::config::ModelConfig;

pub struct Embedder {
    model: TextEmbedding,
    dimensions: usize,
    batch_size: usize,
}

impl Embedder {
    pub fn load(config: &ModelConfig) -> Result<Self> {
        let cache_dir = config.cache_dir.clone().unwrap_or_else(|| {
            dirs_cache().join("code-abyss").join("models")
        });
        std::fs::create_dir_all(&cache_dir)?;

        let embedding_model = resolve_model(&config.model_id);

        info!(
            "loading embedding model: {} ({}d)",
            config.model_id, config.dimensions
        );

        let model = TextEmbedding::try_new(
            InitOptions::new(embedding_model)
                .with_cache_dir(cache_dir)
                .with_show_download_progress(true),
        )
        .with_context(|| format!("failed to load model: {}", config.model_id))?;

        info!("model loaded successfully");

        Ok(Self {
            model,
            dimensions: config.dimensions,
            batch_size: config.batch_size,
        })
    }

    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let results = self.model.embed(vec![text], None)?;
        results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty embedding result"))
    }

    pub fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut all_embeddings = Vec::with_capacity(texts.len());

        for batch_start in (0..texts.len()).step_by(self.batch_size) {
            let batch_end = (batch_start + self.batch_size).min(texts.len());
            let batch: Vec<&str> = texts[batch_start..batch_end].to_vec();
            let embeddings = self.model.embed(batch, None)?;
            all_embeddings.extend(embeddings);
        }

        Ok(all_embeddings)
    }

    pub fn dimensions(&self) -> usize {
        self.dimensions
    }
}

fn resolve_model(model_id: &str) -> EmbeddingModel {
    match model_id {
        "jinaai/jina-embeddings-v2-base-code" => EmbeddingModel::JinaEmbeddingsV2BaseCode,
        "BAAI/bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
        "BAAI/bge-base-en-v1.5" => EmbeddingModel::BGEBaseENV15,
        "BAAI/bge-large-en-v1.5" => EmbeddingModel::BGELargeENV15,
        "sentence-transformers/all-MiniLM-L6-v2" => EmbeddingModel::AllMiniLML6V2,
        _ => {
            // Default to jina code model
            EmbeddingModel::JinaEmbeddingsV2BaseCode
        }
    }
}

fn dirs_cache() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}
