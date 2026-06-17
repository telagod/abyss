use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use tracing::{debug, info, warn};

use super::chunker::{ChunkKind, Chunker, CodeChunk};
use super::parser::{self, MultiParser};
use super::walker::FileWalker;
use crate::config::Config;
use crate::embedding::Embedder;
use crate::graph::extractor::RawReference;
use crate::graph::languages;
use crate::storage::Repository;

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
    generated: bool,
}

/// Workspace-relative path with unix separators — the canonical form stored in
/// the index. Keeps dir-based resolution tiers working on Windows.
fn to_rel(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub struct IndexPipeline {
    config: Config,
    parser: MultiParser,
    chunker: Chunker,
    max_files: Option<u64>,
}

impl IndexPipeline {
    pub fn new(config: Config) -> Self {
        Self {
            chunker: Chunker::new(100, 3),
            parser: MultiParser::new(),
            config,
            max_files: None,
        }
    }

    pub fn set_max_files(&mut self, n: u64) {
        self.max_files = Some(n);
    }

    /// Fast structural index only: parse + chunk + symbols + FTS5.
    /// Returns immediately, no embedding. Searchable via symbols + fulltext.
    pub fn run_structural(&self, repo: &Repository) -> Result<IndexStats> {
        let start = Instant::now();
        let walker = FileWalker::new(&self.config.workspace);
        let files = walker.walk()?;
        info!("found {} indexable files", files.len());

        if let Some(limit) = self.max_files
            && files.len() as u64 > limit
        {
            anyhow::bail!(
                "found {} indexable files (limit {}). This looks like an unscoped directory. \
                 Use --force to proceed or --max-files 0 to disable this check.",
                files.len(),
                limit,
            );
        }

        let existing = repo.all_file_paths()?;
        let existing_map: std::collections::HashMap<String, (i64, String)> = existing
            .into_iter()
            .map(|(id, path, hash)| (path, (id, hash)))
            .collect();

        let mut stats = IndexStats::default();
        let mut to_index: Vec<(std::path::PathBuf, String)> = Vec::new();

        for path in &files {
            let rel_path = to_rel(&self.config.workspace, path);

            let content = match std::fs::read(path) {
                Ok(b) => b,
                Err(e) => {
                    debug!("skip {}: {e}", rel_path);
                    stats.skipped += 1;
                    continue;
                }
            };

            let hash = blake3::hash(&content).to_hex().to_string();
            if let Some((_, eh)) = existing_map.get(&rel_path)
                && *eh == hash
            {
                stats.unchanged += 1;
                continue;
            }
            to_index.push((path.clone(), rel_path));
        }

        // Deleted files
        let current_paths: std::collections::HashSet<String> = files
            .iter()
            .map(|p| to_rel(&self.config.workspace, p))
            .collect();
        for (path, (id, _)) in &existing_map {
            if !current_paths.contains(path) {
                repo.delete_file(*id)?;
                stats.deleted += 1;
            }
        }

        info!(
            "to index: {}, unchanged: {}, deleted: {}",
            to_index.len(),
            stats.unchanged,
            stats.deleted
        );

        // ═══ Launch git log parse in background (IO only, no DB) ═══
        let git_workspace = self.config.workspace.clone();
        let git_handle = std::thread::spawn(move || {
            crate::temporal::git_parser::parse_git_log_to_memory(&git_workspace, 90)
        });

        // ═══ Parallel parse (CPU-bound) ═══
        use rayon::prelude::*;

        let workspace = self.config.workspace.clone();
        let index_generated = self.config.index.index_generated;
        let outputs: Vec<FileOutput> = to_index
            .par_iter()
            .filter_map(|(path, rel_path)| {
                Self::process_file_parallel(&workspace, rel_path, path, index_generated).ok()
            })
            .collect();

        let parse_ms = start.elapsed().as_millis() as u64;

        // ═══ Batch insert (IO-bound, prepared statements) ═══
        let mut total_refs = 0u64;

        repo.begin_transaction()?;
        for output in &outputs {
            match self.insert_file_output(repo, output) {
                Ok(r) => {
                    stats.indexed += 1;
                    stats.chunks += output.chunks.len() as u64;
                    total_refs += r;
                }
                Err(e) => {
                    warn!("insert failed {}: {e}", output.rel_path);
                    stats.errors += 1;
                }
            }
        }
        repo.commit()?;

        let insert_ms = start.elapsed().as_millis() as u64 - parse_ms;

        // ═══ Batch resolve refs ═══
        self.resolve_import_bindings(repo)?;
        self.batch_resolve_refs(repo)?;
        stats.refs = total_refs;

        let resolve_ms = start.elapsed().as_millis() as u64 - parse_ms - insert_ms;

        // ═══ Wait for git parse + write to DB + compute metrics ═══
        let git_data = git_handle
            .join()
            .map_err(|_| anyhow::anyhow!("git thread panicked"))??;
        let git_stats = crate::temporal::git_parser::write_git_data(repo, &git_data)?;

        crate::temporal::hotspot::compute_file_metrics(repo, 30, 90)?;
        crate::temporal::coupling::compute_change_coupling(repo, 4)?;

        // ═══ Step 9: L0 architectural coordinates ═══
        //
        // Fuse dictionary / naming / entry-point / topology signals into one
        // ArchFact per file, then derive Louvain modules from the same graph.
        // The card's `where` line reads from arch_facts; populating it here
        // keeps the cost in the indexer (one transaction per pass) rather
        // than the hot read path of every hook invocation.
        //
        // Load user overrides (`.code-abyss/arch.toml`) from the configured
        // workspace — not cwd — so the indexer behaves correctly when invoked
        // from outside the project root (e.g. CI runners, MCP server with a
        // pinned workspace).
        let arch_start = Instant::now();
        let arch_overrides = crate::arch::load_overrides(&self.config.workspace);
        let arch_facts = crate::arch::inference::infer_all_with_overrides(
            repo,
            Some(&self.config.workspace),
            arch_overrides.as_ref(),
        )?;
        repo.replace_arch_facts(&arch_facts)?;
        let arch_modules = crate::arch::inference::collect_modules(&arch_facts);
        repo.replace_arch_modules(&arch_modules)?;
        let arch_ms = arch_start.elapsed().as_millis() as u64;

        stats.total_files = repo.file_count()? as u64;
        stats.total_chunks = repo.chunk_count()? as u64;
        stats.total_symbols = repo.symbol_count()? as u64;
        stats.arch_files = arch_facts.len() as u64;
        stats.arch_modules = arch_modules.len() as u64;
        stats.duration_ms = start.elapsed().as_millis() as u64;

        info!(
            "done in {}ms | parse {}ms | insert {}ms | resolve {}ms | arch {}ms ({} files / {} modules) | git {} commits (overlapped)",
            stats.duration_ms,
            parse_ms,
            insert_ms,
            resolve_ms,
            arch_ms,
            stats.arch_files,
            stats.arch_modules,
            git_stats.commits_parsed
        );

        Ok(stats)
    }

    /// Pure CPU work — no DB access, safe for parallel execution
    fn process_file_parallel(
        _workspace: &Path,
        rel_path: &str,
        path: &Path,
        index_generated: bool,
    ) -> Result<FileOutput> {
        let source = std::fs::read_to_string(path)?;
        let hash = blake3::hash(source.as_bytes()).to_hex().to_string();
        let language = parser::detect_language(rel_path);
        let generated = parser::is_generated(&source);
        let mtime = std::fs::metadata(path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let size = source.len() as i64;

        // Parse once (thread-local parser)
        let parser = MultiParser::new();
        let tree = language.as_deref().and_then(|lang| {
            if parser.supports(lang) {
                parser.parse(&source, lang).ok()
            } else {
                None
            }
        });

        // Chunks + symbols
        let chunker = Chunker::new(100, 3);
        let chunks = if let Some(ref tree) = tree {
            chunker.chunk(&source, tree, language.as_deref().unwrap_or(""))
        } else {
            vec![CodeChunk {
                content: source.clone(),
                kind: ChunkKind::Module,
                start_line: 0,
                end_line: source.lines().count().saturating_sub(1) as u32,
                scope: None,
                symbols: Vec::new(),
            }]
        };

        // Refs + complexity (same tree)
        let mut refs = Vec::new();
        let mut complexity = 0.0f64;
        let mut max_func_lines = 0u32;

        if let (Some(tree), Some(lang)) = (&tree, &language) {
            // Generated code keeps its symbols (chunks above) so hand-written
            // callers still resolve, but its own call edges are mechanical
            // noise — skip ref extraction unless explicitly opted in.
            if (!generated || index_generated)
                && let Some(extractor) = languages::get_extractor(lang)
            {
                refs = extractor.extract(tree, &source);
            }
            complexity =
                crate::temporal::complexity::cyclomatic_complexity(tree, &source, lang) as f64;
            max_func_lines = crate::temporal::complexity::max_function_lines(tree, &source, lang);
        }

        Ok(FileOutput {
            rel_path: rel_path.to_string(),
            hash,
            language,
            mtime,
            size,
            chunks,
            refs,
            complexity,
            max_func_lines,
            generated,
        })
    }

    fn insert_file_output(&self, repo: &Repository, out: &FileOutput) -> Result<u64> {
        if let Some(old_id) = repo.get_file_id(&out.rel_path)? {
            repo.delete_refs_from_file(old_id)?;
            repo.delete_chunks_for_file(old_id)?;
            repo.delete_symbols_for_file(old_id)?;
            repo.delete_file(old_id)?;
        }

        let file_id = repo.upsert_file(
            &out.rel_path,
            &out.hash,
            out.language.as_deref(),
            out.mtime,
            out.size,
            out.generated,
        )?;
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
                    file_id,
                    &chunk.content,
                    chunk.kind.as_str(),
                    chunk.start_line,
                    chunk.end_line,
                    chunk.scope.as_deref(),
                    tc
                ])?;
                let chunk_id = conn.last_insert_rowid();
                for sym in &chunk.symbols {
                    sym_stmt.execute(rusqlite::params![
                        chunk_id,
                        file_id,
                        &sym.name,
                        sym.kind.as_str(),
                        sym.line,
                        sym.scope.as_deref().or(chunk.scope.as_deref())
                    ])?;
                }
            }
        }

        // Refs — prepared statement loop (faster than dynamic multi-row SQL)
        let ref_count = out.refs.len() as u64;
        {
            let mut ref_stmt = conn.prepare_cached(
                "INSERT INTO refs(source_file_id,source_line,source_symbol,target_name,target_qualifier,receiver_type,kind,confidence) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)")?;
            for raw in &out.refs {
                ref_stmt.execute(rusqlite::params![
                    file_id,
                    raw.line,
                    raw.source_symbol.as_deref(),
                    &raw.target_name,
                    raw.target_qualifier.as_deref(),
                    raw.receiver_type.as_deref(),
                    raw.kind.as_str(),
                    0.0f64
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

    /// Resolve `import_binding` refs to file ids, entirely against the files
    /// table (no disk probing): relative module paths are normalized against
    /// the importing file's dir and matched with the usual TS/JS extension
    /// and index-file candidates. Then barrel chains are chased: a binding
    /// that lands on a file with no same-named symbol but a same-named
    /// binding (re-export / import-then-export) is retargeted to where that
    /// binding points, up to a fixed depth.
    fn resolve_import_bindings(&self, repo: &Repository) -> Result<()> {
        let conn = repo.conn();

        let mut paths: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT path, id FROM files")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (p, id) = row?;
                paths.insert(p, id);
            }
        }

        let bindings: Vec<(i64, i64, String, String, String, Option<String>)> = {
            let mut stmt = conn.prepare(
                "SELECT r.id, r.source_file_id, f.dir, r.target_name, r.target_qualifier,
                        f.language
                 FROM refs r
                 JOIN files f ON r.source_file_id = f.id
                 WHERE r.kind = 'import_binding' AND r.target_file_id IS NULL
                   AND r.target_qualifier IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            })?;
            rows.collect::<std::result::Result<_, _>>()?
        };

        conn.execute_batch("BEGIN")?;
        {
            let mut update = conn.prepare("UPDATE refs SET target_file_id = ?1 WHERE id = ?2")?;
            let mut own_symbol =
                conn.prepare("SELECT 1 FROM symbols WHERE file_id = ?1 AND name = ?2 LIMIT 1")?;
            for (id, src_fid, dir, name, module, language) in bindings {
                let fid = match language.as_deref() {
                    Some("python") => resolve_py_module(&dir, &module, &paths),
                    Some("java") => resolve_java_class(&module, &paths),
                    Some("rust") => {
                        // `mod tests { use super::escape; }` — super inside an
                        // INLINE module is the file itself. Bindings don't
                        // record module nesting, so: if the source file
                        // defines the item, bind to it before any dir logic.
                        if module.starts_with("super::")
                            && own_symbol.exists(rusqlite::params![src_fid, name])?
                        {
                            Some(src_fid)
                        } else {
                            resolve_rust_use(&dir, &module, &paths)
                        }
                    }
                    // TS/JS: only relative imports resolve in-repo; package
                    // imports stay NULL.
                    _ if module.starts_with('.') => {
                        let base = normalize_rel_path(&format!("{dir}{module}"));
                        resolve_module_file(&base, &paths)
                    }
                    _ => None,
                };
                if let Some(fid) = fid {
                    update.execute(rusqlite::params![fid, id])?;
                }
            }
        }
        conn.execute_batch("COMMIT")?;

        // Barrel chase: bounded fixpoint, each pass follows one re-export hop.
        for _ in 0..5 {
            let changed = conn.execute(
                "UPDATE refs SET target_file_id = (
                     SELECT ib.target_file_id FROM refs ib
                     WHERE ib.source_file_id = refs.target_file_id
                       AND ib.kind = 'import_binding'
                       AND ib.target_name = refs.target_name
                       AND ib.target_file_id IS NOT NULL
                       AND ib.target_file_id != refs.target_file_id
                     LIMIT 1)
                 WHERE kind = 'import_binding' AND target_file_id IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM symbols s
                       WHERE s.file_id = refs.target_file_id AND s.name = refs.target_name)
                   AND EXISTS (SELECT 1 FROM refs ib
                       WHERE ib.source_file_id = refs.target_file_id
                         AND ib.kind = 'import_binding'
                         AND ib.target_name = refs.target_name
                         AND ib.target_file_id IS NOT NULL
                         AND ib.target_file_id != refs.target_file_id)",
                [],
            )?;
            if changed == 0 {
                break;
            }
        }

        Ok(())
    }

    fn batch_resolve_refs(&self, repo: &Repository) -> Result<()> {
        let conn = repo.conn();

        // Update query planner stats for better index usage
        conn.execute_batch("ANALYZE symbols; ANALYZE refs; ANALYZE files;")?;

        // Level 0: Receiver-type match (confidence = 0.95).
        // The call site knows its receiver's static type (x.M() where x: T,
        // inferred lite from receivers/params/local literals) and exactly one
        // file defines a same-named symbol owned by that type (symbols.scope).
        // Runs BEFORE same-file: type evidence beats proximity — same-file
        // name reuse on a different type was a measured error class.
        let l0 = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT s.file_id FROM symbols s
                     WHERE s.name = refs.target_name AND s.scope = refs.receiver_type LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s
                     WHERE s.name = refs.target_name AND s.scope = refs.receiver_type LIMIT 1),
                 confidence = 0.95
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND receiver_type IS NOT NULL
               AND (SELECT COUNT(DISTINCT s.file_id) FROM symbols s
                   WHERE s.name = refs.target_name AND s.scope = refs.receiver_type) = 1",
            [],
        )?;

        // Level 0c: Typed receiver via the TYPE's import binding (0.95).
        // L0's exact scope match dies on type aliases (`use X as Separator`),
        // trait-scoped methods, and impls split across files — but when the
        // source file IMPORTS the receiver type, the binding's target file is
        // type-grade evidence. Require the method name to exist there.
        let l0c = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT ib.target_file_id FROM refs ib
                     WHERE ib.source_file_id = refs.source_file_id
                       AND ib.kind = 'import_binding'
                       AND ib.target_name = refs.receiver_type
                       AND ib.target_file_id IS NOT NULL LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s
                     WHERE s.name = refs.target_name
                       AND s.file_id = (SELECT ib.target_file_id FROM refs ib
                           WHERE ib.source_file_id = refs.source_file_id
                             AND ib.kind = 'import_binding'
                             AND ib.target_name = refs.receiver_type
                             AND ib.target_file_id IS NOT NULL LIMIT 1)
                     LIMIT 1),
                 confidence = 0.95
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND receiver_type IS NOT NULL
               AND EXISTS (SELECT 1 FROM refs ib
                   JOIN symbols s ON s.file_id = ib.target_file_id
                   WHERE ib.source_file_id = refs.source_file_id
                     AND ib.kind = 'import_binding'
                     AND ib.target_name = refs.receiver_type
                     AND ib.target_file_id IS NOT NULL
                     AND s.name = refs.target_name)",
            [],
        )?;

        // Level 0d: Typed receiver via the type's unique defining file
        // (0.95). The type symbol (class/struct/interface/enum) lives in
        // exactly one file and that file defines the method name — methods
        // and their type overwhelmingly share a file.
        let l0d = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT t.file_id FROM symbols t
                     WHERE t.name = refs.receiver_type
                       AND t.kind IN ('class', 'struct', 'interface', 'enum') LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s
                     WHERE s.name = refs.target_name
                       AND s.file_id = (SELECT t.file_id FROM symbols t
                           WHERE t.name = refs.receiver_type
                             AND t.kind IN ('class', 'struct', 'interface', 'enum') LIMIT 1)
                     LIMIT 1),
                 confidence = 0.95
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND receiver_type IS NOT NULL
               AND (SELECT COUNT(DISTINCT t.file_id) FROM symbols t
                   WHERE t.name = refs.receiver_type
                     AND t.kind IN ('class', 'struct', 'interface', 'enum')) = 1
               AND EXISTS (SELECT 1 FROM symbols m
                   WHERE m.name = refs.target_name
                     AND m.file_id = (SELECT t.file_id FROM symbols t
                         WHERE t.name = refs.receiver_type
                           AND t.kind IN ('class', 'struct', 'interface', 'enum') LIMIT 1))",
            [],
        )?;

        // Level 0b: Named-import binding (confidence = 0.95). A bare call
        // whose name is bound by `import { x } from './mod'` resolves to the
        // module's file — the strongest evidence short of a compiler, and it
        // runs BEFORE same-file: hono's `css()` is imported from helper/css
        // while an unrelated `css` symbol lives elsewhere; global-unique
        // claimed the wrong file 45×. Barrel chains were already chased at
        // binding-resolution time.
        let l0b = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT ib.target_file_id FROM refs ib
                     WHERE ib.source_file_id = refs.source_file_id
                       AND ib.kind = 'import_binding'
                       AND ib.target_name = refs.target_name
                       AND ib.target_file_id IS NOT NULL LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s
                     WHERE s.name = refs.target_name
                       AND s.file_id = (SELECT ib.target_file_id FROM refs ib
                           WHERE ib.source_file_id = refs.source_file_id
                             AND ib.kind = 'import_binding'
                             AND ib.target_name = refs.target_name
                             AND ib.target_file_id IS NOT NULL LIMIT 1)
                     LIMIT 1),
                 confidence = 0.95
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND target_qualifier IS NULL
               AND EXISTS (SELECT 1 FROM refs ib
                   WHERE ib.source_file_id = refs.source_file_id
                     AND ib.kind = 'import_binding'
                     AND ib.target_name = refs.target_name
                     AND ib.target_file_id IS NOT NULL)",
            [],
        )?;

        // Level 1: Same-file (confidence = 1.0) — uses idx_symbols_name_file.
        // Typed-receiver refs are L0's exclusive territory: if the receiver
        // type is known but L0 found no owned symbol (dynamic methods,
        // unindexed owners), proximity is a measured-bad guess — demote
        // instead. Eval on hono: app.get() (runtime-assigned method) was
        // claimed same-file 185×.
        //
        // Qualified calls (x.foo() with an unknown receiver) are excluded too:
        // measured across gin/hono/click, qualified same-file matches with a
        // non-unique name were 23.5% precision (31 correct / 101 wrong) —
        // common names (get/route) get claimed by unrelated same-file symbols
        // (object-literal Proxy traps, other classes' methods). Bare calls are
        // 99.6% and self-like receivers (this/self/cls/super()) 98.4% — only
        // those keep the 1.0 tier. Self-like receivers are exempt from the
        // typed-receiver exclusion: self.m() where L0 found no owned symbol
        // is usually an INHERITED method (click: ParamType.fail called from
        // every subclass), and base + subclass overwhelmingly share a file.
        // Qualified leftovers fall through to the qualifier/global tiers and,
        // failing those, the 0.6 same-file fallback below the gate.
        let l1 = conn.execute(
            "UPDATE refs SET
                 target_file_id = source_file_id,
                 target_symbol_id = (SELECT s.id FROM symbols s
                     WHERE s.name = refs.target_name AND s.file_id = refs.source_file_id LIMIT 1),
                 confidence = 1.0
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND ((receiver_type IS NULL AND target_qualifier IS NULL)
                    OR target_qualifier IN ('this', 'self', 'cls', 'super')
                    OR target_qualifier GLOB 'super(*')
               AND EXISTS (SELECT 1 FROM symbols s
                   WHERE s.name = refs.target_name AND s.file_id = refs.source_file_id)",
            [],
        )?;

        // Level 2: Same-package with a UNIQUE candidate file (confidence = 0.95).
        // Multi-candidate same-package matches (interface-method name collisions:
        // many types in one package defining Render/String/Bind) are NOT resolved
        // here — they fall through to the qualifier tier and, failing that, to the
        // demoted 0.6 tier below. Eval on gin showed these collisions dominate
        // resolution errors (see eval/RESULTS.md).
        //
        // Rust only: qualified calls (x.m(), unknown receiver) are excluded —
        // a Rust dir is NOT a namespace (files in one dir are separate
        // modules needing `use`), so dir proximity is weak evidence there:
        // measured 76% on ripgrep vs 98% on gin (Go dirs ARE packages).
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
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND receiver_type IS NULL
               AND (target_qualifier IS NULL
                    OR COALESCE((SELECT language FROM files
                        WHERE id = refs.source_file_id), '') != 'rust')
               AND (SELECT COUNT(DISTINCT s.file_id) FROM symbols s JOIN files f ON s.file_id = f.id
                   WHERE s.name = refs.target_name
                     AND f.dir = (SELECT dir FROM files WHERE id = refs.source_file_id)
                     AND s.file_id != refs.source_file_id) = 1",
            [],
        )?;

        // Level 3: Import-qualifier match with a UNIQUE candidate file (confidence = 0.9).
        // Multi-file qualifier matches (e.g. build-tag variants all defining the
        // same symbol) fall through to the demoted tiers.
        // `util.Fn()` resolves to a file in a dir named `util/` (or file `util.ext`)
        // when the source file imports a path whose last segment is `util`.
        // Disambiguates same-named symbols across packages before the global tiers.
        let l3q = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT s.file_id FROM symbols s JOIN files f ON s.file_id = f.id
                     WHERE s.name = refs.target_name
                       AND (f.dir GLOB '*' || refs.target_qualifier || '/'
                            OR f.path GLOB '*/' || refs.target_qualifier || '.*'
                            OR f.path GLOB refs.target_qualifier || '.*')
                     LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s JOIN files f ON s.file_id = f.id
                     WHERE s.name = refs.target_name
                       AND (f.dir GLOB '*' || refs.target_qualifier || '/'
                            OR f.path GLOB '*/' || refs.target_qualifier || '.*'
                            OR f.path GLOB refs.target_qualifier || '.*')
                     LIMIT 1),
                 confidence = 0.9
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND target_qualifier IS NOT NULL
               AND EXISTS (SELECT 1 FROM refs ir
                   WHERE ir.source_file_id = refs.source_file_id AND ir.kind = 'import'
                     AND (ir.target_name = refs.target_qualifier
                          OR ir.target_name GLOB '*/' || refs.target_qualifier
                          OR ir.target_name GLOB '*.' || refs.target_qualifier))
               AND (SELECT COUNT(DISTINCT s.file_id) FROM symbols s JOIN files f ON s.file_id = f.id
                   WHERE s.name = refs.target_name
                     AND (f.dir GLOB '*' || refs.target_qualifier || '/'
                          OR f.path GLOB '*/' || refs.target_qualifier || '.*'
                          OR f.path GLOB refs.target_qualifier || '.*')) = 1",
            [],
        )?;

        // Level 4: Global unique (confidence = 0.8).
        // Qualified calls (x.foo()) may only take a global-unique candidate
        // that looks like a member (a method, or scoped to an owner type):
        // measured on hono, x.foo() resolving to an unscoped free function
        // was 6% precision (app.use() → the JSX `use` hook, 47×), while
        // member-shaped candidates were 96.7%.
        let l3 = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT s.file_id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 confidence = 0.8
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND receiver_type IS NULL
               AND (SELECT COUNT(DISTINCT file_id) FROM symbols WHERE name = refs.target_name) = 1
               AND (target_qualifier IS NULL
                    OR EXISTS (SELECT 1 FROM symbols s
                        WHERE s.name = refs.target_name
                          AND (s.scope IS NOT NULL OR s.kind = 'method')))",
            [],
        )?;

        // Level 4a: Same-file fallback for qualified calls (confidence = 0.6).
        // x.foo() with an unknown receiver and a same-file candidate that the
        // earlier tiers didn't claim: still the best single guess, but
        // measured-bad (23.5% on common names) — surfaced as a possible, not
        // a fact, below the 0.7 gate.
        let l4a = conn.execute(
            "UPDATE refs SET
                 target_file_id = source_file_id,
                 target_symbol_id = (SELECT s.id FROM symbols s
                     WHERE s.name = refs.target_name AND s.file_id = refs.source_file_id LIMIT 1),
                 confidence = 0.6
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND EXISTS (SELECT 1 FROM symbols s
                   WHERE s.name = refs.target_name AND s.file_id = refs.source_file_id)",
            [],
        )?;

        // Level 4b: Same-package, multiple candidates (confidence = 0.6).
        // Below the default 0.7 gate: surfaced as possible_callers, not facts.
        let l2b = conn.execute(
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
                 confidence = 0.6
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND EXISTS (SELECT 1 FROM symbols s JOIN files f ON s.file_id = f.id
                   WHERE s.name = refs.target_name
                     AND f.dir = (SELECT dir FROM files WHERE id = refs.source_file_id)
                     AND s.file_id != refs.source_file_id)",
            [],
        )?;

        // Level 5: Ambiguous global (confidence = 0.5)
        let l4 = conn.execute(
            "UPDATE refs SET
                 target_file_id = (SELECT s.file_id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 target_symbol_id = (SELECT s.id FROM symbols s WHERE s.name = refs.target_name LIMIT 1),
                 confidence = 0.5
             WHERE confidence = 0.0 AND kind NOT IN ('import', 'import_binding')
               AND EXISTS (SELECT 1 FROM symbols WHERE name = refs.target_name)",
            [],
        )?;

        info!(
            "resolved: L0(receiver-type)={}, L0c(type-binding)={}, L0d(type-file)={}, L0b(import-binding)={}, L1(same-file)={}, L2(same-pkg-unique)={}, L3(qualifier)={}, L4(global-unique)={}, L4a(same-file-qual)={}, L4b(same-pkg-multi)={}, L5(ambiguous)={}",
            l0, l0c, l0d, l0b, l1, l2, l3q, l3, l4a, l2b, l4
        );

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
            return Ok(EmbedStats {
                total: 0,
                embedded: 0,
                skipped: 0,
                duration_ms: 0,
            });
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
        info!(
            "embedding {} chunks (skipping {} non-code/trivial)",
            embed_count, skipped
        );

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
            for ((chunk_id, _), vec) in to_embed[batch_start..batch_end].iter().zip(vectors.iter())
            {
                repo.insert_vector(*chunk_id, vec)?;
            }
            repo.commit()?;

            embedded += (batch_end - batch_start) as u64;
            if embedded.is_multiple_of(256) || batch_end == to_embed.len() {
                let elapsed = start.elapsed().as_secs();
                let rate = embedded.checked_div(elapsed).unwrap_or(embedded);
                let remaining = (embed_count as u64 - embedded)
                    .checked_div(rate)
                    .unwrap_or(0);
                info!(
                    "  embedded {}/{} ({}/s, ~{}s remaining)",
                    embedded, embed_count, rate, remaining
                );
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

    fn index_file_structural(&self, repo: &Repository, path: &Path, rel_path: &str) -> Result<u64> {
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

        let generated = parser::is_generated(&source);
        let file_id =
            repo.upsert_file(rel_path, &hash, language.as_deref(), mtime, size, generated)?;
        let chunks = self.parse_and_chunk(&source, language.as_deref());

        let mut count = 0u64;
        for chunk in &chunks {
            let token_count = chunk.content.split_whitespace().count() as u32;
            let chunk_id = repo.insert_chunk(
                file_id,
                &chunk.content,
                chunk.kind.as_str(),
                chunk.start_line,
                chunk.end_line,
                chunk.scope.as_deref(),
                token_count,
            )?;

            for sym in &chunk.symbols {
                repo.insert_symbol(
                    chunk_id,
                    file_id,
                    &sym.name,
                    sym.kind.as_str(),
                    sym.line,
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
            && let Ok(tree) = self.parser.parse(source, lang)
        {
            let chunks = self.chunker.chunk(source, &tree, lang);
            if !chunks.is_empty() {
                return chunks;
            }
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
        &self,
        repo: &Repository,
        embedder: &Embedder,
        path: &Path,
        rel_path: &str,
    ) -> Result<u64> {
        let count = self.index_file_structural(repo, path, rel_path)?;

        // Immediate embedding for single file (incremental update)
        if let Some(file_id) = repo.get_file_id(rel_path)? {
            let chunks = repo.chunks_for_file(file_id)?;
            let embeddable: Vec<_> = chunks
                .iter()
                .filter(|c| {
                    let lang = parser::detect_language(rel_path);
                    is_embeddable_language(lang.as_deref())
                        && c.kind != "import"
                        && c.token_count >= 8
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
        let rel_path = to_rel(&self.config.workspace, path);
        self.index_file(repo, embedder, path, &rel_path)
    }

    pub fn remove_file(&self, repo: &Repository, path: &Path) -> Result<()> {
        let rel_path = to_rel(&self.config.workspace, path);
        if let Some(id) = repo.get_file_id(&rel_path)? {
            repo.delete_file(id)?;
        }
        Ok(())
    }
}

fn is_embeddable_language(lang: Option<&str>) -> bool {
    matches!(
        lang,
        Some(
            "rust"
                | "python"
                | "javascript"
                | "typescript"
                | "tsx"
                | "go"
                | "java"
                | "c"
                | "cpp"
                | "bash"
        )
    )
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
    /// Number of files that received a fused L0 ArchFact in this pass.
    pub arch_files: u64,
    /// Number of Louvain communities the file→file graph collapsed into.
    pub arch_modules: u64,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct EmbedStats {
    pub total: u64,
    pub embedded: u64,
    pub skipped: u64,
    pub duration_ms: u64,
}

/// Collapse `.` / `..` segments in a repo-relative path (no filesystem).
fn normalize_rel_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Python module → file: relative (`.mod`, `..pkg.mod`) resolves against the
/// importing file's dir (one leading dot = current package, each extra dot =
/// one level up); absolute dotted paths (`click.types`) match exactly or by
/// unique path suffix (handles `src/` layouts). Candidates: `<base>.py`,
/// `<base>/__init__.py`.
fn resolve_py_module(
    dir: &str,
    module: &str,
    paths: &std::collections::HashMap<String, i64>,
) -> Option<i64> {
    let base = if module.starts_with('.') {
        let dots = module.len() - module.trim_start_matches('.').len();
        let rest = module.trim_start_matches('.');
        let mut d = dir.trim_end_matches('/');
        for _ in 1..dots {
            d = match d.rfind('/') {
                Some(pos) => &d[..pos],
                None => "",
            };
        }
        match (d.is_empty(), rest.is_empty()) {
            (true, _) => rest.replace('.', "/"),
            (false, true) => d.to_string(),
            (false, false) => format!("{d}/{}", rest.replace('.', "/")),
        }
    } else {
        module.replace('.', "/")
    };
    if base.is_empty() {
        return None;
    }
    for cand in [format!("{base}.py"), format!("{base}/__init__.py")] {
        if let Some(&id) = paths.get(&cand) {
            return Some(id);
        }
        // src-layout etc.: unique suffix match only — ambiguity stays NULL.
        let suffix = format!("/{cand}");
        let mut hit = None;
        for (p, &id) in paths {
            if p.ends_with(&suffix) {
                if hit.is_some() {
                    hit = None;
                    break;
                }
                hit = Some(id);
            }
        }
        if hit.is_some() {
            return hit;
        }
    }
    None
}

/// Rust `use` path → the file defining the MODULE that owns the bound item.
/// `crate::graph::extractor::RawReference` → src/graph/extractor.rs (or
/// .../extractor/mod.rs); `super::chunker::Chunker` resolves against the
/// importing file's dir (first `super` = the dir itself, each extra one =
/// one level up). Bare first segments try dir-relative, then crate-root,
/// then unique path suffix. `pub use` re-export hops are the barrel chase's
/// job, not this function's.
fn resolve_rust_use(
    dir: &str,
    use_path: &str,
    paths: &std::collections::HashMap<String, i64>,
) -> Option<i64> {
    let segs: Vec<&str> = use_path.split("::").collect();
    if segs.is_empty() {
        return None;
    }
    // The last segment is the bound ITEM; the module path is what maps to a
    // file.
    let module_segs = &segs[..segs.len() - 1];

    let try_module = |base: &str,
                      rest: &[&str],
                      paths: &std::collections::HashMap<String, i64>|
     -> Option<i64> {
        if rest.is_empty() {
            // Item at crate root: lib.rs / main.rs
            for root in ["lib.rs", "main.rs"] {
                let cand = if base.is_empty() {
                    root.to_string()
                } else {
                    format!("{base}/{root}")
                };
                if let Some(&id) = paths.get(&cand) {
                    return Some(id);
                }
            }
            return None;
        }
        let joined = rest.join("/");
        let stem = if base.is_empty() {
            joined
        } else {
            format!("{base}/{}", rest.join("/"))
        };
        for cand in [format!("{stem}.rs"), format!("{stem}/mod.rs")] {
            if let Some(&id) = paths.get(&cand) {
                return Some(id);
            }
        }
        None
    };

    match module_segs.first() {
        Some(&"crate") => {
            // Crate root: walk the importing file's ancestor dirs for a
            // Cargo.toml (workspace members like crates/cli/src/...), trying
            // <member>/src then <member> itself (path-overridden roots like
            // ripgrep's crates/core/main.rs). Plain src/ is the fallback.
            let mut d = dir.trim_end_matches('/');
            loop {
                let toml = if d.is_empty() {
                    "Cargo.toml".to_string()
                } else {
                    format!("{d}/Cargo.toml")
                };
                if paths.contains_key(&toml) {
                    let src = if d.is_empty() {
                        "src".to_string()
                    } else {
                        format!("{d}/src")
                    };
                    if let Some(id) = try_module(&src, &module_segs[1..], paths)
                        .or_else(|| try_module(d, &module_segs[1..], paths))
                    {
                        return Some(id);
                    }
                }
                if d.is_empty() {
                    break;
                }
                d = match d.rfind('/') {
                    Some(pos) => &d[..pos],
                    None => "",
                };
            }
            try_module("src", &module_segs[1..], paths)
                .or_else(|| try_module("", &module_segs[1..], paths))
        }
        Some(&"super") => {
            // First super = the importing file's dir (its parent module),
            // each additional super = one level up.
            let supers = module_segs.iter().take_while(|s| **s == "super").count();
            let mut d = dir.trim_end_matches('/');
            for _ in 1..supers {
                d = match d.rfind('/') {
                    Some(pos) => &d[..pos],
                    None => "",
                };
            }
            try_module(d, &module_segs[supers..], paths)
        }
        Some(&"self") => try_module(dir.trim_end_matches('/'), &module_segs[1..], paths),
        Some(_) => {
            // Bare path: dir-relative submodule, then crate-root module,
            // then unique suffix (workspace layouts).
            try_module(dir.trim_end_matches('/'), module_segs, paths)
                .or_else(|| try_module("src", module_segs, paths))
                .or_else(|| {
                    let suffix = format!("/{}.rs", module_segs.join("/"));
                    let mut hit = None;
                    for (p, &id) in paths {
                        if p.ends_with(&suffix) {
                            if hit.is_some() {
                                return None;
                            }
                            hit = Some(id);
                        }
                    }
                    hit
                })
        }
        // `use crate::Item` style: module_segs == ["crate"] handled above;
        // empty module path (use Item;) — extern prelude, unresolvable.
        None => None,
    }
}

/// Java `import com.foo.Bar` → the unique file whose path ends with
/// `/com/foo/Bar.java` (or equals `com/foo/Bar.java`). Ambiguity stays NULL.
fn resolve_java_class(
    import_path: &str,
    paths: &std::collections::HashMap<String, i64>,
) -> Option<i64> {
    let cand = format!("{}.java", import_path.replace('.', "/"));
    if let Some(&id) = paths.get(&cand) {
        return Some(id);
    }
    let suffix = format!("/{cand}");
    let mut hit = None;
    for (p, &id) in paths {
        if p.ends_with(&suffix) {
            if hit.is_some() {
                return None;
            }
            hit = Some(id);
        }
    }
    hit
}

/// Match a normalized module base against indexed file paths, trying the
/// TS/JS resolution candidates: exact, with extensions, ESM `.js`→`.ts`
/// rewrites, and directory index files.
fn resolve_module_file(base: &str, paths: &std::collections::HashMap<String, i64>) -> Option<i64> {
    if let Some(&id) = paths.get(base) {
        return Some(id);
    }
    // ESM-style `./x.js` source written in TS → x.ts / x.tsx
    for (from, to) in [
        (".js", ".ts"),
        (".js", ".tsx"),
        (".jsx", ".tsx"),
        (".mjs", ".mts"),
    ] {
        if let Some(stem) = base.strip_suffix(from)
            && let Some(&id) = paths.get(&format!("{stem}{to}"))
        {
            return Some(id);
        }
    }
    for ext in [".ts", ".tsx", ".js", ".jsx", ".mts", ".cts"] {
        if let Some(&id) = paths.get(&format!("{base}{ext}")) {
            return Some(id);
        }
    }
    for idx in ["/index.ts", "/index.tsx", "/index.js", "/index.jsx"] {
        if let Some(&id) = paths.get(&format!("{base}{idx}")) {
            return Some(id);
        }
    }
    None
}
