use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use tracing::{info, warn, debug};

use crate::config::Config;
use crate::embedding::Embedder;
use crate::graph::languages;
use crate::graph::extractor::RawReference;
use crate::storage::Repository;
use super::chunker::{Chunker, CodeChunk, ChunkKind};
use super::parser::{self, MultiParser};
use super::walker::FileWalker;

struct FileOutput {
    rel_path: String,
    hash: String,
    language: Option<String>,
    mtime: i64,
    size: i64,
    chunks: Vec<CodeChunk>,
    refs: Vec<RawReference>,
    complexity: f64,
    max_func_lines: u32,
}

pub struct IndexPipeline {
    config: Config,
    parser: MultiParser,
    chunker: Chunker,
}

impl IndexPipeline {
    pub fn new(config: Config) -> Self {
        Self {
            chunker: Chunker::new(100, 3),
            parser: MultiParser::new(),
            config,
        }
    }

    /// Fast structural index only: parse + chunk + symbols + FTS5.
    /// Returns immediately, no embedding. Searchable via symbols + fulltext.
    pub fn run_structural(&self, repo: &Repository) -> Result<IndexStats> {
        let start = Instant::now();
        let walker = FileWalker::new(&self.config.workspace);
        let files = walker.walk()?;
        info!("found {} indexable files", files.len());

        let existing = repo.all_file_paths()?;
        let existing_map: std::collections::HashMap<String, (i64, String)> = existing
            .into_iter()
            .map(|(id, path, hash)| (path, (id, hash)))
            .collect();

        let mut stats = IndexStats::default();
        let mut to_index: Vec<(std::path::PathBuf, String)> = Vec::new();

        for path in &files {
            let rel_path = path
                .strip_prefix(&self.config.workspace)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let content = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => { debug!("skip {}: {e}", rel_path); stats.skipped += 1; continue; }
            };

            let hash = blake3::hash(&content).to_hex().to_string();
            if let Some((_, eh)) = existing_map.get(&rel_path)
                && *eh == hash { stats.unchanged += 1; continue; }
            to_index.push((path.clone(), rel_path));
        }

        // Deleted files
        let current_paths: std::collections::HashSet<String> = files
            .iter()
            .filter_map(|p| p.strip_prefix(&self.config.workspace).ok().map(|r| r.to_string_lossy().to_string()))
            .collect();
        for (path, (id, _)) in &existing_map {
            if !current_paths.contains(path) { repo.delete_file(*id)?; stats.deleted += 1; }
        }

        info!("to index: {}, unchanged: {}, deleted: {}", to_index.len(), stats.unchanged, stats.deleted);

        // ═══ Launch git log parse in background (IO only, no DB) ═══
        let git_workspace = self.config.workspace.clone();
        let git_handle = std::thread::spawn(move || {
            crate::temporal::git_parser::parse_git_log_to_memory(&git_workspace, 90)
        });

        // ═══ Parallel parse (CPU-bound) ═══
        use rayon::prelude::*;

        let workspace = self.config.workspace.clone();
        let outputs: Vec<FileOutput> = to_index
            .par_iter()
            .filter_map(|(path, rel_path)| {
                Self::process_file_parallel(&workspace, rel_path, path).ok()
            })
            .collect();

        let parse_ms = start.elapsed().as_millis() as u64;

        // ═══ Batch insert (IO-bound, prepared statements) ═══
        let mut total_refs = 0u64;

        repo.begin_transaction()?;
        for output in &outputs {
            match self.insert_file_output(repo, output) {
                Ok(r) => { stats.indexed += 1; stats.chunks += output.chunks.len() as u64; total_refs += r; }
                Err(e) => { warn!("insert failed {}: {e}", output.rel_path); stats.errors += 1; }
            }
        }
        repo.commit()?;

        let insert_ms = start.elapsed().as_millis() as u64 - parse_ms;

        // ═══ Batch resolve refs ═══
        self.batch_resolve_refs(repo)?;
        stats.refs = total_refs;

        let resolve_ms = start.elapsed().as_millis() as u64 - parse_ms - insert_ms;

        // ═══ Wait for git parse + write to DB + compute metrics ═══
        let git_data = git_handle.join().map_err(|_| anyhow::anyhow!("git thread panicked"))??;
        let git_stats = crate::temporal::git_parser::write_git_data(repo, &git_data)?;

        crate::temporal::hotspot::compute_file_metrics(repo, 30, 90)?;
        crate::temporal::coupling::compute_change_coupling(repo, 3)?;

        stats.total_files = repo.file_count()? as u64;
        stats.total_chunks = repo.chunk_count()? as u64;
        stats.total_symbols = repo.symbol_count()? as u64;
        stats.duration_ms = start.elapsed().as_millis() as u64;

        info!("done in {}ms | parse {}ms | insert {}ms | resolve {}ms | git {} commits (overlapped)",
            stats.duration_ms, parse_ms, insert_ms, resolve_ms, git_stats.commits_parsed);

        Ok(stats)
    }

    /// Pure CPU work — no DB access, safe for parallel execution
    fn process_file_parallel(
        _workspace: &Path,
        rel_path: &str,
        path: &Path,
    ) -> Result<FileOutput> {
        let source = std::fs::read_to_string(path)?;
        let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
        let language = parser::detect_language(rel_path);
        let mtime = std::fs::metadata(path)?.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64;
        let size = source.len() as i64;

        // Parse once (thread-local parser)
        let parser = MultiParser::new();
        let tree = language.as_deref().and_then(|lang| {
            if parser.supports(lang) { parser.parse(&source, lang).ok() } else { None }
        });

        // Chunks + symbols
        let chunker = Chunker::new(100, 3);
        let chunks = if let Some(ref tree) = tree {
            chunker.chunk(&source, tree, language.as_deref().unwrap_or(""))
        } else {
            vec![CodeChunk {
                content: source.clone(), kind: ChunkKind::Module,
                start_line: 0, end_line: source.lines().count().saturating_sub(1) as u32,
                scope: None, symbols: Vec::new(),
            }]
        };

        // Refs + complexity (same tree)
        let mut refs = Vec::new();
        let mut complexity = 0.0f64;
        let mut max_func_lines = 0u32;

        if let (Some(tree), Some(lang)) = (&tree, &language) {
            if let Some(extractor) = languages::get_extractor(lang) {
                refs = extractor.extract(tree, &source);
            }
            complexity = crate::temporal::complexity::cyclomatic_complexity(tree, &source, lang) as f64;
            max_func_lines = crate::temporal::complexity::max_function_lines(tree, &source, lang);
        }

        Ok(FileOutput {
            rel_path: rel_path.to_string(),
            hash, language, mtime, size, chunks, refs, complexity, max_func_lines,
        })
    }

    fn insert_file_output(&self, repo: &Repository, out: &FileOutput) -> Result<u64> {
        if let Some(old_id) = repo.get_file_id(&out.rel_path)? {
            repo.delete_refs_from_file(old_id)?;
            repo.delete_chunks_for_file(old_id)?;
            repo.delete_symbols_for_file(old_id)?;
            repo.delete_file(old_id)?;
        }

        let file_id = repo.upsert_file(&out.rel_path, &out.hash, out.language.as_deref(), out.mtime, out.size)?;
        let conn = repo.conn();

        // Prepared statements — compiled once, reused per file
        {
            let mut chunk_stmt = conn.prepare_cached(
                "INSERT INTO chunks(file_id,content,kind,start_line,end_line,scope,token_count) VALUES(?1,?2,?3,?4,?5,?6,?7)")?;
            let mut sym_stmt = conn.prepare_cached(
                "INSERT INTO symbols(chunk_id,file_id,name,kind,line,scope) VALUES(?1,?2,?3,?4,?5,?6)")?;

            for chunk in &out.chunks {
                let tc = chunk.content.split_whitespace().count() as u32;
                chunk_stmt.execute(rusqlite::params![
                    file_id, &chunk.content, chunk.kind.as_str(),
                    chunk.start_line, chunk.end_line, chunk.scope.as_deref(), tc
                ])?;
                let chunk_id = conn.last_insert_rowid();
                for sym in &chunk.symbols {
                    sym_stmt.execute(rusqlite::params![
                        chunk_id, file_id, &sym.name, sym.kind.as_str(), sym.line, chunk.scope.as_deref()
                    ])?;
                }
            }
        }

        // Refs — prepared statement loop (faster than dynamic multi-row SQL)
        let ref_count = out.refs.len() as u64;
        {
            let mut ref_stmt = conn.prepare_cached(
                "INSERT INTO refs(source_file_id,source_line,source_symbol,target_name,target_qualifier,kind,confidence) VALUES(?1,?2,?3,?4,?5,?6,?7)")?;
            for raw in &out.refs {
                ref_stmt.execute(rusqlite::params![
                    file_id, raw.line, raw.source_symbol.as_deref(),
                    &raw.target_name, raw.target_qualifier.as_deref(),
                    raw.kind.as_str(), 0.0f64
                ])?;
            }
        }

        // Complexity
        if out.complexity > 0.0 {
            conn.execute(
                "INSERT OR REPLACE INTO file_metrics(file_id,cyclomatic,max_func_lines) VALUES(?1,?2,?3)",
                rusqlite::params![file_id, out.complexity, out.max_func_lines],
            )?;
        }

        Ok(ref_count)
    }

    fn batch_resolve_refs(&self, repo: &Repository) -> Result<()> {
        let conn = repo.conn();

        // Update query planner stats for better index usage
        conn.execute_batch("ANALYZE symbols; ANALYZE refs; ANALYZE files;")?;

        // Level 1: Same-file (confidence = 1.0) — uses idx_symbols_name_file
        let l1 = conn.execute(
            "UPDATE refs SET
                 target_file_id = source_file_id,
                 target_symbol_id = (SELECT s.id FROM symbols s
                     WHERE s.name = refs.target_name AND s.file_id = refs.source_file_id LIMIT 1),
                 confidence = 1.0
             WHERE confidence = 0.0 AND kind != 'import'
               AND EXISTS (SELECT 1 FROM symbols s
                   WHERE s.name = refs.target_name AND s.file_id = refs.source_file_id)",
            [],
        )?;

        // Level 2: Same-package (confidence = 0.95) — uses idx_files_dir + idx_symbols_name_file
        let l2 = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT s.file_id FROM symbols s
                     JOIN files f ON s.file_id = f.id
                     WHERE s.name = refs.target_name
                       AND f.dir = (SELECT dir FROM files WHERE id = refs.source_file_id)
                       AND s.file_id != refs.source_file_id
                     LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s
                     JOIN files f ON s.file_id = f.id
                     WHERE s.name = refs.target_name
                       AND f.dir = (SELECT dir FROM files WHERE id = refs.source_file_id)
                       AND s.file_id != refs.source_file_id
                     LIMIT 1),
                 confidence = 0.95
             WHERE confidence = 0.0 AND kind != 'import'
               AND EXISTS (SELECT 1 FROM symbols s JOIN files f ON s.file_id = f.id
                   WHERE s.name = refs.target_name
                     AND f.dir = (SELECT dir FROM files WHERE id = refs.source_file_id)
                     AND s.file_id != refs.source_file_id)",
            [],
        )?;

        // Level 3: Global unique (confidence = 0.8)
        let l3 = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT s.file_id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 confidence = 0.8
             WHERE confidence = 0.0 AND kind != 'import'
               AND (SELECT COUNT(DISTINCT file_id) FROM symbols WHERE name = refs.target_name) = 1",
            [],
        )?;

        // Level 4: Ambiguous global (confidence = 0.5)
        let l4 = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT s.file_id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 confidence = 0.5
             WHERE confidence = 0.0 AND kind != 'import'
               AND EXISTS (SELECT 1 FROM symbols WHERE name = refs.target_name)",
            [],
        )?;

        info!("resolved: L1(same-file)={}, L2(same-pkg)={}, L3(global-unique)={}, L4(ambiguous)={}",
            l1, l2, l3, l4);

        Ok(())
    }

    /// Embed all un-embedded chunks. Can be called separately after run_structural.
    pub fn run_embedding(&self, repo: &Repository, embedder: &Embedder) -> Result<EmbedStats> {
        let start = Instant::now();

        // Find chunks without vectors
        let unembedded = repo.unembedded_chunk_ids()?;
        let total = unembedded.len();

        if total == 0 {
            info!("all chunks already embedded");
            return Ok(EmbedStats { total: 0, embedded: 0, skipped: 0, duration_ms: 0 });
        }

        // Filter: only embed code chunks, skip config/data
        let mut to_embed: Vec<(i64, String)> = Vec::new();
        let mut skipped = 0u64;

        for chunk_id in &unembedded {
            if let Some(chunk) = repo.get_chunk(*chunk_id)? {
                let file_path = repo.get_file_path(chunk.file_id)?.unwrap_or_default();
                let lang = parser::detect_language(&file_path);
                let embeddable = is_embeddable_language(lang.as_deref())
                    && chunk.kind != "import"
                    && chunk.token_count >= 8;

                if embeddable {
                    to_embed.push((*chunk_id, chunk.content));
                } else {
                    skipped += 1;
                }
            }
        }

        let embed_count = to_embed.len();
        info!("embedding {} chunks (skipping {} non-code/trivial)", embed_count, skipped);

        let batch_size = self.config.model.batch_size;
        let mut embedded = 0u64;

        for batch_start in (0..to_embed.len()).step_by(batch_size) {
            let batch_end = (batch_start + batch_size).min(to_embed.len());
            let texts: Vec<&str> = to_embed[batch_start..batch_end]
                .iter()
                .map(|(_, c)| c.as_str())
                .collect();

            let vectors = embedder.embed_batch(&texts)?;

            repo.begin_transaction()?;
            for ((chunk_id, _), vec) in to_embed[batch_start..batch_end].iter().zip(vectors.iter()) {
                repo.insert_vector(*chunk_id, vec)?;
            }
            repo.commit()?;

            embedded += (batch_end - batch_start) as u64;
            if embedded.is_multiple_of(256) || batch_end == to_embed.len() {
                let elapsed = start.elapsed().as_secs();
                let rate = embedded.checked_div(elapsed).unwrap_or(embedded);
                let remaining = (embed_count as u64 - embedded).checked_div(rate).unwrap_or(0);
                info!("  embedded {}/{} ({}/s, ~{}s remaining)", embedded, embed_count, rate, remaining);
            }
        }

        Ok(EmbedStats {
            total: total as u64,
            embedded,
            skipped,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Full run: structural + embedding (for convenience / backward compat)
    pub fn run(&self, repo: &Repository, embedder: &Embedder) -> Result<IndexStats> {
        let mut stats = self.run_structural(repo)?;
        let embed_stats = self.run_embedding(repo, embedder)?;
        stats.embed_duration_ms = embed_stats.duration_ms;
        Ok(stats)
    }

    fn index_file_structural(
        &self,
        repo: &Repository,
        path: &Path,
        rel_path: &str,
    ) -> Result<u64> {
        let source = std::fs::read_to_string(path)?;
        let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
        let language = parser::detect_language(rel_path);
        let mtime = std::fs::metadata(path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let size = source.len() as i64;

        if let Some(old_id) = repo.get_file_id(rel_path)? {
            repo.delete_chunks_for_file(old_id)?;
            repo.delete_symbols_for_file(old_id)?;
            repo.delete_file(old_id)?;
        }

        let file_id = repo.upsert_file(rel_path, &hash, language.as_deref(), mtime, size)?;
        let chunks = self.parse_and_chunk(&source, language.as_deref());

        let mut count = 0u64;
        for chunk in &chunks {
            let token_count = chunk.content.split_whitespace().count() as u32;
            let chunk_id = repo.insert_chunk(
                file_id, &chunk.content, chunk.kind.as_str(),
                chunk.start_line, chunk.end_line, chunk.scope.as_deref(), token_count,
            )?;

            for sym in &chunk.symbols {
                repo.insert_symbol(
                    chunk_id, file_id, &sym.name, sym.kind.as_str(), sym.line,
                    chunk.scope.as_deref(),
                )?;
            }
            count += 1;
        }

        Ok(count)
    }

    fn parse_and_chunk(&self, source: &str, language: Option<&str>) -> Vec<CodeChunk> {
        if let Some(lang) = language
            && self.parser.supports(lang)
                && let Ok(tree) = self.parser.parse(source, lang) {
                    let chunks = self.chunker.chunk(source, &tree, lang);
                    if !chunks.is_empty() { return chunks; }
                }
        vec![CodeChunk {
            content: source.to_string(),
            kind: ChunkKind::Module,
            start_line: 0,
            end_line: source.lines().count().saturating_sub(1) as u32,
            scope: None,
            symbols: Vec::new(),
        }]
    }

    pub fn index_file(
        &self, repo: &Repository, embedder: &Embedder, path: &Path, rel_path: &str,
    ) -> Result<u64> {
        let count = self.index_file_structural(repo, path, rel_path)?;

        // Immediate embedding for single file (incremental update)
        if let Some(file_id) = repo.get_file_id(rel_path)? {
            let chunks = repo.chunks_for_file(file_id)?;
            let embeddable: Vec<_> = chunks.iter()
                .filter(|c| {
                    let lang = parser::detect_language(rel_path);
                    is_embeddable_language(lang.as_deref())
                        && c.kind != "import" && c.token_count >= 8
                })
                .collect();

            if !embeddable.is_empty() {
                let texts: Vec<&str> = embeddable.iter().map(|c| c.content.as_str()).collect();
                let vectors = embedder.embed_batch(&texts)?;
                for (c, vec) in embeddable.iter().zip(vectors.iter()) {
                    repo.insert_vector(c.id, vec)?;
                }
            }
        }

        Ok(count)
    }

    pub fn reindex_file(&self, repo: &Repository, embedder: &Embedder, path: &Path) -> Result<u64> {
        let rel_path = path.strip_prefix(&self.config.workspace).unwrap_or(path)
            .to_string_lossy().to_string();
        self.index_file(repo, embedder, path, &rel_path)
    }

    pub fn remove_file(&self, repo: &Repository, path: &Path) -> Result<()> {
        let rel_path = path.strip_prefix(&self.config.workspace).unwrap_or(path)
            .to_string_lossy().to_string();
        if let Some(id) = repo.get_file_id(&rel_path)? { repo.delete_file(id)?; }
        Ok(())
    }
}

fn is_embeddable_language(lang: Option<&str>) -> bool {
    matches!(lang, Some("rust" | "python" | "javascript" | "typescript" | "tsx" | "go" | "java" | "c" | "cpp" | "bash"))
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct IndexStats {
    pub indexed: u64,
    pub unchanged: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub errors: u64,
    pub chunks: u64,
    pub total_files: u64,
    pub total_chunks: u64,
    pub total_symbols: u64,
    pub refs: u64,
    pub duration_ms: u64,
    pub embed_duration_ms: u64,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct EmbedStats {
    pub total: u64,
    pub embedded: u64,
    pub skipped: u64,
    pub duration_ms: u64,
}
