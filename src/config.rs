use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub workspace: PathBuf,
    pub db_path: PathBuf,
    pub model: ModelConfig,
    pub index: IndexConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_id: String,
    pub cache_dir: Option<PathBuf>,
    pub batch_size: usize,
    pub dimensions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    pub max_chunk_tokens: usize,
    pub min_chunk_tokens: usize,
    pub watch: bool,
    pub languages: Vec<String>,
}

impl Config {
    pub fn new(workspace: impl AsRef<Path>) -> Self {
        let workspace = workspace.as_ref().to_path_buf();
        let db_path = workspace.join(".code-abyss").join("index.db");
        Self {
            workspace,
            db_path,
            model: ModelConfig::default(),
            index: IndexConfig::default(),
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_id: "jinaai/jina-embeddings-v2-base-code".to_string(),
            cache_dir: None,
            batch_size: 128,
            dimensions: 768,
        }
    }
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            max_chunk_tokens: 512,
            min_chunk_tokens: 32,
            watch: true,
            languages: vec![],
        }
    }
}
