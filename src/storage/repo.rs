use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, ffi::sqlite3_auto_extension, params};

use super::schema;

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub hash: String,
    pub language: Option<String>,
    pub mtime: i64,
    pub size: i64,
}

#[derive(Debug, Clone)]
pub struct ChunkRecord {
    pub id: i64,
    pub file_id: i64,
    pub content: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub scope: Option<String>,
    pub token_count: u32,
}

#[derive(Debug, Clone)]
pub struct SymbolRecord {
    pub id: i64,
    pub chunk_id: i64,
    pub file_id: i64,
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub scope: Option<String>,
}

pub struct Repository {
    conn: Connection,
    _dimensions: usize,
}

impl Repository {
    pub fn open(db_path: &Path, dimensions: usize) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create db dir: {}", parent.display()))?;
        }

        // Register sqlite-vec as auto extension BEFORE opening connection
        #[allow(clippy::missing_transmute_annotations)]
        // FFI signature fixed by sqlite3_auto_extension
        unsafe {
            sqlite3_auto_extension(Some(std::mem::transmute(
                sqlite_vec::sqlite3_vec_init as *const (),
            )));
        }

        let conn = Connection::open(db_path)
            .with_context(|| format!("failed to open db: {}", db_path.display()))?;

        schema::init_db(&conn)?;
        schema::init_vec_table(&conn, dimensions)?;

        Ok(Self {
            conn,
            _dimensions: dimensions,
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    // --- File operations ---

    pub fn upsert_file(
        &self,
        path: &str,
        hash: &str,
        language: Option<&str>,
        mtime: i64,
        size: i64,
        generated: bool,
    ) -> Result<i64> {
        let dir = match path.rfind('/') {
            Some(pos) => &path[..pos + 1],
            None => "",
        };
        let lang_family = language_family(language);
        self.conn.execute(
            "INSERT INTO files(path, hash, language, mtime, size, dir, generated, lang_family)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path) DO UPDATE SET hash=?2, language=?3, mtime=?4, size=?5, dir=?6, generated=?7, lang_family=?8",
            params![path, hash, language, mtime, size, dir, generated as i64, lang_family],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_file_by_path(&self, path: &str) -> Result<Option<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, hash, language, mtime, size FROM files WHERE path = ?1")?;
        let mut rows = stmt.query_map([path], |row| {
            Ok(FileRecord {
                id: row.get(0)?,
                path: row.get(1)?,
                hash: row.get(2)?,
                language: row.get(3)?,
                mtime: row.get(4)?,
                size: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn get_file_id(&self, path: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT id FROM files WHERE path = ?1", [path], |r| r.get(0))
            .ok())
    }

    /// Resolve a user-supplied file argument to a single (id, path) tuple,
    /// preferring the most-specific reasonable match. Used by `abyss where`,
    /// `abyss context`, and the pre-edit hook so an agent typing
    /// `src/hono.ts` lands on the root file, not `benchmarks/jsx/src/hono.ts`.
    ///
    /// Priority:
    /// 1. Exact `path = query` match.
    /// 2. Paths starting with `query` (root-anchored), shortest first.
    /// 3. Paths ending with `query` (suffix match), shortest first.
    ///
    /// Returns `None` when nothing matches.
    pub fn find_file_fuzzy(&self, query: &str) -> Result<Option<(i64, String)>> {
        // 1. Exact match — cheap, short-circuit.
        if let Ok(row) =
            self.conn
                .query_row("SELECT id, path FROM files WHERE path = ?1", [query], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
                })
        {
            return Ok(Some(row));
        }

        // 2 & 3. Prefer prefix match over suffix match; within each bucket,
        // shortest path wins (fewest directory hops). The CASE-when ordering
        // is the priority bucket; LENGTH(path) breaks ties — for suffix
        // matches that means the root-level `src/hono.ts` beats a deeply
        // nested `benchmarks/jsx/src/hono.ts`.
        let prefix_pattern = format!("{query}%");
        let suffix_pattern = format!("%{query}");
        let row = self.conn.query_row(
            "SELECT id, path FROM files
             WHERE path LIKE ?1 OR path LIKE ?2
             ORDER BY
                 CASE WHEN path LIKE ?1 THEN 0 ELSE 1 END,
                 LENGTH(path)
             LIMIT 1",
            params![prefix_pattern, suffix_pattern],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        );
        Ok(row.ok())
    }

    pub fn delete_file(&self, file_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM vec_chunks WHERE chunk_id IN (SELECT id FROM chunks WHERE file_id = ?1)",
            [file_id],
        )?;
        self.conn
            .execute("DELETE FROM files WHERE id = ?1", [file_id])?;
        Ok(())
    }

    pub fn all_file_paths(&self) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare("SELECT id, path, hash FROM files")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // --- Chunk operations ---

    #[allow(clippy::too_many_arguments)]
    pub fn insert_chunk(
        &self,
        file_id: i64,
        content: &str,
        kind: &str,
        start_line: u32,
        end_line: u32,
        scope: Option<&str>,
        token_count: u32,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO chunks(file_id, content, kind, start_line, end_line, scope, token_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                file_id,
                content,
                kind,
                start_line,
                end_line,
                scope,
                token_count
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_chunks_for_file(&self, file_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM vec_chunks WHERE chunk_id IN (SELECT id FROM chunks WHERE file_id = ?1)",
            [file_id],
        )?;
        self.conn
            .execute("DELETE FROM chunks WHERE file_id = ?1", [file_id])?;
        Ok(())
    }

    // --- Symbol operations ---

    pub fn insert_symbol(
        &self,
        chunk_id: i64,
        file_id: i64,
        name: &str,
        kind: &str,
        line: u32,
        scope: Option<&str>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO symbols(chunk_id, file_id, name, kind, line, scope)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![chunk_id, file_id, name, kind, line, scope],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_symbols_for_file(&self, file_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM symbols WHERE file_id = ?1", [file_id])?;
        Ok(())
    }

    // --- Vector operations ---

    pub fn insert_vector(&self, chunk_id: i64, embedding: &[f32]) -> Result<()> {
        let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.conn.execute(
            "INSERT INTO vec_chunks(chunk_id, embedding) VALUES (?1, ?2)",
            params![chunk_id, blob],
        )?;
        Ok(())
    }

    pub fn search_vectors(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<(i64, f64)>> {
        let blob: Vec<u8> = query_embedding
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let mut stmt = self.conn.prepare(
            "SELECT chunk_id, distance
             FROM vec_chunks
             WHERE embedding MATCH ?1
             ORDER BY distance
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![blob, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // --- Query helpers ---

    pub fn get_chunk(&self, chunk_id: i64) -> Result<Option<ChunkRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_id, content, kind, start_line, end_line, scope, token_count
             FROM chunks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([chunk_id], |row| {
            Ok(ChunkRecord {
                id: row.get(0)?,
                file_id: row.get(1)?,
                content: row.get(2)?,
                kind: row.get(3)?,
                start_line: row.get(4)?,
                end_line: row.get(5)?,
                scope: row.get(6)?,
                token_count: row.get(7)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn get_file_path(&self, file_id: i64) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT path FROM files WHERE id = ?1", [file_id], |r| {
                r.get(0)
            })
            .ok())
    }

    pub fn file_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?)
    }

    pub fn chunk_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?)
    }

    pub fn symbol_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?)
    }

    pub fn unembedded_chunk_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id FROM chunks c
             LEFT JOIN vec_chunks v ON v.chunk_id = c.id
             WHERE v.chunk_id IS NULL",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn chunks_for_file(&self, file_id: i64) -> Result<Vec<ChunkRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, file_id, content, kind, start_line, end_line, scope, token_count
             FROM chunks WHERE file_id = ?1",
        )?;
        let rows = stmt.query_map([file_id], |row| {
            Ok(ChunkRecord {
                id: row.get(0)?,
                file_id: row.get(1)?,
                content: row.get(2)?,
                kind: row.get(3)?,
                start_line: row.get(4)?,
                end_line: row.get(5)?,
                scope: row.get(6)?,
                token_count: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn has_vectors(&self) -> Result<bool> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM vec_chunks LIMIT 1", [], |r| r.get(0))?;
        Ok(count > 0)
    }

    // --- Reference (graph) operations ---

    #[allow(clippy::too_many_arguments)]
    pub fn insert_ref(
        &self,
        source_file_id: i64,
        source_line: u32,
        source_symbol: Option<&str>,
        target_name: &str,
        target_qualifier: Option<&str>,
        target_file_id: Option<i64>,
        target_symbol_id: Option<i64>,
        kind: &str,
        confidence: f64,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO refs(source_file_id, source_line, source_symbol, target_name,
              target_qualifier, target_file_id, target_symbol_id, kind, confidence)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                source_file_id,
                source_line,
                source_symbol,
                target_name,
                target_qualifier,
                target_file_id,
                target_symbol_id,
                kind,
                confidence
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn delete_refs_from_file(&self, file_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM refs WHERE source_file_id = ?1", [file_id])?;
        Ok(())
    }

    /// Legacy caller lookup: `('call','field_access')` only. Used by
    /// `impact_analysis` (BFS over invocations, not type positions).
    ///
    /// For agent-facing `callers` queries, prefer
    /// [`Self::find_callers_of_kinds`] — type-position users (interface
    /// implementers, generic instantiations, `extends` clauses) are real
    /// dependencies and showing them by default matches what the agent means
    /// by "who depends on this".
    pub fn find_callers_of(
        &self,
        target_name: &str,
        target_file_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<RefRecord>> {
        self.find_callers_of_kinds(
            target_name,
            target_file_id,
            &["call", "field_access"],
            limit,
        )
    }

    /// Caller lookup with a caller-specified kind filter. `kinds` is the SQL
    /// `IN (...)` set used for `refs.kind`.
    ///
    /// Empty `kinds` is treated as the legacy default
    /// (`('call','field_access')`) so callers can't accidentally turn off
    /// every filter and pull `import`/`import_binding` rows.
    pub fn find_callers_of_kinds(
        &self,
        target_name: &str,
        target_file_id: Option<i64>,
        kinds: &[&str],
        limit: usize,
    ) -> Result<Vec<RefRecord>> {
        let kinds: Vec<&str> = if kinds.is_empty() {
            vec!["call", "field_access"]
        } else {
            kinds.to_vec()
        };
        // Build the IN list inline. Kinds come from a closed set of static
        // strings (CLI flag dispatch + MCP enum), so no untrusted input
        // reaches the SQL builder — we can safely interpolate.
        debug_assert!(
            kinds
                .iter()
                .all(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')),
            "kinds must be SQL-safe ascii idents, got {kinds:?}"
        );
        let placeholders: Vec<String> = kinds.iter().map(|k| format!("'{k}'")).collect();
        let kinds_sql = placeholders.join(",");

        let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<RefRecord> {
            Ok(RefRecord {
                id: row.get(0)?,
                source_file_id: row.get(1)?,
                source_line: row.get(2)?,
                source_symbol: row.get(3)?,
                target_name: row.get(4)?,
                kind: row.get(5)?,
                confidence: row.get(6)?,
                source_file_path: row.get(7)?,
            })
        };

        if let Some(fid) = target_file_id {
            let sql = format!(
                "SELECT r.id, r.source_file_id, r.source_line, r.source_symbol,
                        r.target_name, r.kind, r.confidence, f.path
                 FROM refs r JOIN files f ON r.source_file_id = f.id
                 WHERE r.target_name = ?1 AND (r.target_file_id = ?2 OR r.target_file_id IS NULL)
                 AND r.kind IN ({kinds_sql})
                 ORDER BY r.confidence DESC LIMIT ?3"
            );
            let mut s = self.conn.prepare(&sql)?;
            return Ok(s
                .query_map(params![target_name, fid, limit as i64], map_row)?
                .filter_map(|r| r.ok())
                .collect());
        }
        let sql = format!(
            "SELECT r.id, r.source_file_id, r.source_line, r.source_symbol,
                    r.target_name, r.kind, r.confidence, f.path
             FROM refs r JOIN files f ON r.source_file_id = f.id
             WHERE r.target_name = ?1
             AND r.kind IN ({kinds_sql})
             ORDER BY r.confidence DESC LIMIT ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![target_name, limit as i64], map_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Total visible caller refs for the "showing N of M" footer (B3).
    /// Returns the count of refs whose `target_name` matches, `kind` is in
    /// the filter set, and `confidence >= min_confidence`. When
    /// `include_tests=false`, refs originating from test files are excluded
    /// so M matches the denominator the agent actually sees.
    ///
    /// Snapshot of *visible* callers, not DB-wide — a footer "showing 20 of
    /// 50" where 30 of those 50 are hidden test callers would be more
    /// misleading than helpful. Test detection mixes path regexes + content
    /// sniffing so it stays Rust-side (cheap: one symbol's caller rowset).
    pub fn count_callers_at(
        &self,
        target_name: &str,
        kinds: &[&str],
        min_confidence: f64,
        include_tests: bool,
    ) -> Result<usize> {
        let kinds: Vec<&str> = if kinds.is_empty() {
            vec!["call", "field_access"]
        } else {
            kinds.to_vec()
        };
        debug_assert!(
            kinds
                .iter()
                .all(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')),
            "kinds must be SQL-safe ascii idents, got {kinds:?}"
        );
        let placeholders: Vec<String> = kinds.iter().map(|k| format!("'{k}'")).collect();
        let kinds_sql = placeholders.join(",");

        if include_tests {
            let sql = format!(
                "SELECT COUNT(*) FROM refs r
                 WHERE r.target_name = ?1
                   AND r.kind IN ({kinds_sql})
                   AND r.confidence >= ?2"
            );
            let n: i64 = self
                .conn
                .query_row(&sql, params![target_name, min_confidence], |r| r.get(0))?;
            return Ok(n as usize);
        }
        let sql = format!(
            "SELECT r.source_file_id FROM refs r
             WHERE r.target_name = ?1
               AND r.kind IN ({kinds_sql})
               AND r.confidence >= ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![target_name, min_confidence], |r| r.get::<_, i64>(0))?;
        let mut n = 0usize;
        for fid in rows.flatten() {
            if !self.is_test_file(fid).unwrap_or(false) {
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn find_refs_from_file(&self, file_id: i64) -> Result<Vec<RefRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.source_file_id, r.source_line, r.source_symbol,
                    r.target_name, r.kind, r.confidence, f.path
             FROM refs r JOIN files f ON r.source_file_id = f.id
             WHERE r.source_file_id = ?1",
        )?;
        let rows = stmt.query_map([file_id], |row| {
            Ok(RefRecord {
                id: row.get(0)?,
                source_file_id: row.get(1)?,
                source_line: row.get(2)?,
                source_symbol: row.get(3)?,
                target_name: row.get(4)?,
                kind: row.get(5)?,
                confidence: row.get(6)?,
                source_file_path: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn ref_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM refs", [], |r| r.get(0))?)
    }

    pub fn find_symbols_in_file(&self, file_id: i64) -> Result<Vec<SymbolRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, chunk_id, file_id, name, kind, line, scope
             FROM symbols WHERE file_id = ?1 AND kind IN ('function','method','struct','class','interface','type')
             ORDER BY line")?;
        let rows = stmt.query_map([file_id], |row| {
            Ok(SymbolRecord {
                id: row.get(0)?,
                chunk_id: row.get(1)?,
                file_id: row.get(2)?,
                name: row.get(3)?,
                kind: row.get(4)?,
                line: row.get(5)?,
                scope: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn find_symbol_by_name_in_file(
        &self,
        file_id: i64,
        name: &str,
    ) -> Result<Option<SymbolRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, chunk_id, file_id, name, kind, line, scope
             FROM symbols WHERE file_id = ?1 AND name = ?2 LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![file_id, name], |row| {
            Ok(SymbolRecord {
                id: row.get(0)?,
                chunk_id: row.get(1)?,
                file_id: row.get(2)?,
                name: row.get(3)?,
                kind: row.get(4)?,
                line: row.get(5)?,
                scope: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn find_symbol_global(&self, name: &str) -> Result<Vec<SymbolRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, chunk_id, file_id, name, kind, line, scope
             FROM symbols WHERE name = ?1",
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok(SymbolRecord {
                id: row.get(0)?,
                chunk_id: row.get(1)?,
                file_id: row.get(2)?,
                name: row.get(3)?,
                kind: row.get(4)?,
                line: row.get(5)?,
                scope: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn files_in_directory(&self, dir: &str) -> Result<Vec<i64>> {
        let pattern = format!("{dir}%");
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM files WHERE path LIKE ?1 AND path NOT LIKE ?2")?;
        let subdir_pattern = format!("{dir}%/%");
        let rows = stmt.query_map(params![pattern, subdir_pattern], |row| row.get(0))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn is_test_file(&self, file_id: i64) -> Result<bool> {
        let path: String =
            self.conn
                .query_row("SELECT path FROM files WHERE id = ?1", [file_id], |r| {
                    r.get(0)
                })?;
        Ok(path.contains("_test.")
            || path.contains("/test/")
            || path.contains("/tests/")
            || path.contains(".test.")
            || path.contains(".spec."))
    }

    pub fn begin_transaction(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN TRANSACTION")?;
        Ok(())
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    pub fn rollback(&self) -> Result<()> {
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct RefRecord {
    pub id: i64,
    pub source_file_id: i64,
    pub source_line: u32,
    pub source_symbol: Option<String>,
    pub target_name: String,
    pub kind: String,
    pub confidence: f64,
    pub source_file_path: String,
}

// ─── Arch facts (L0 coordinates) ─────────────────────────────────────────────

/// Persisted arch_facts row — returned by `Repository::get_arch_fact`.
#[derive(Debug, Clone)]
pub struct ArchFactRow {
    pub file_id: i64,
    pub layer: String,
    pub role: String,
    pub module_id: i64,
    pub depth_from_entry: Option<u32>,
    pub centrality: f64,
    pub in_degree: u32,
    pub out_degree: u32,
    pub layer_conf: f64,
    pub signals: serde_json::Value,
}

/// Persisted arch_modules row — returned by `Repository::get_arch_module`.
#[derive(Debug, Clone)]
pub struct ArchModuleRecord {
    pub id: i64,
    pub label: String,
    pub file_count: i64,
    pub dominant_layer: String,
    pub centroid_path: String,
}

impl Repository {
    /// Replace every row in arch_facts with the given batch, in a single
    /// transaction. Cheaper than an upsert at this scale — the L0 facts are
    /// produced wholesale at the end of every index pass.
    pub fn replace_arch_facts(&self, facts: &[crate::arch::ArchFact]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM arch_facts", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO arch_facts(
                    file_id, layer, role, module_id,
                    depth_from_entry, centrality,
                    in_degree, out_degree, layer_conf, signals)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )?;
            for f in facts {
                let signals_str = f.signals.to_string();
                stmt.execute(params![
                    f.file_id,
                    f.layer,
                    f.role,
                    f.module_id,
                    f.depth_from_entry.map(|d| d as i64),
                    f.centrality,
                    f.in_degree as i64,
                    f.out_degree as i64,
                    f.layer_conf,
                    signals_str,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Replace every row in arch_modules. Labels and centroid paths are
    /// derived here from the file paths of each module's members — the
    /// inference engine only knows file_ids, not paths.
    pub fn replace_arch_modules(&self, modules: &[crate::arch::ArchModuleRow]) -> Result<()> {
        // Pull paths AND centrality for every module member in one query.
        // Centrality is the importance weight used to decide the modal
        // segment label: a community containing `fastapi/applications.py`
        // (high centrality) plus 10 `docs_src/tutorial*.py` (each near
        // zero) labels as "fastapi", not "docs_src" — even though
        // docs_src files dominate by raw count. Raw-count modal labelling
        // was a FastAPI v0.5.0 dogfood regression.
        let mut member_paths: std::collections::HashMap<i64, Vec<(String, f64)>> =
            std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT af.module_id, f.path, af.centrality
                 FROM arch_facts af JOIN files f ON f.id = af.file_id
                 WHERE af.module_id >= 0",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })?;
            for r in rows.flatten() {
                member_paths.entry(r.0).or_default().push((r.1, r.2));
            }
        }

        let tx = self.conn.unchecked_transaction()?;
        // Two-phase label derivation: first compute each module's preferred
        // label, then de-collide so the agent never sees two modules with
        // identical labels (e.g. two communities both deriving as "tests").
        let mut prelim: Vec<(i64, String, String, i64, String)> = Vec::with_capacity(modules.len());
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for m in modules {
            let members = member_paths.get(&m.id).cloned().unwrap_or_default();
            // Longest common prefix is a structural property — every member
            // contributes equally regardless of centrality.
            let only_paths: Vec<String> = members.iter().map(|(p, _)| p.clone()).collect();
            let centroid_path = longest_common_prefix(&only_paths);
            let label = derive_label(&centroid_path, &members, m.id);
            *counts.entry(label.clone()).or_insert(0) += 1;
            prelim.push((
                m.id,
                label,
                centroid_path,
                m.file_count,
                m.dominant_layer.clone(),
            ));
        }
        // Sort so the larger community keeps the un-suffixed label; later
        // collisions get -{module_id} appended (file_count would collide
        // again when N modules have the same size, which is common for
        // tests-only mash-ups).
        prelim.sort_by(|a, b| b.3.cmp(&a.3).then(a.0.cmp(&b.0)));
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut final_labels: Vec<(i64, String, String, i64, String)> =
            Vec::with_capacity(prelim.len());
        for (mid, label, centroid, fcount, dom) in prelim {
            let label_final = if taken.contains(&label) {
                format!("{label}-{mid}")
            } else {
                label
            };
            taken.insert(label_final.clone());
            final_labels.push((mid, label_final, centroid, fcount, dom));
        }

        tx.execute("DELETE FROM arch_modules", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO arch_modules(id, label, file_count, dominant_layer, centroid_path)
                 VALUES (?1,?2,?3,?4,?5)",
            )?;
            for (mid, label, centroid, fcount, dom) in final_labels {
                stmt.execute(params![mid, label, fcount, dom, centroid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Fetch the arch_facts row for a single file, parsing the signals JSON
    /// on the way out so callers don't need to re-parse it.
    pub fn get_arch_fact(&self, file_id: i64) -> Result<Option<ArchFactRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT layer, role, module_id, depth_from_entry,
                    centrality, in_degree, out_degree, layer_conf, signals
             FROM arch_facts WHERE file_id = ?1",
        )?;
        let mut rows = stmt.query_map([file_id], |r| {
            let depth: Option<i64> = r.get(3)?;
            let signals_str: String = r.get(8)?;
            Ok(ArchFactRow {
                file_id,
                layer: r.get(0)?,
                role: r.get(1)?,
                module_id: r.get(2)?,
                depth_from_entry: depth.map(|v| v as u32),
                centrality: r.get(4)?,
                in_degree: r.get::<_, i64>(5)? as u32,
                out_degree: r.get::<_, i64>(6)? as u32,
                layer_conf: r.get(7)?,
                signals: serde_json::from_str(&signals_str).unwrap_or(serde_json::Value::Null),
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Fetch a single arch_modules row by id (used by the card to render a
    /// human-readable module label instead of a numeric id).
    pub fn get_arch_module(&self, module_id: i64) -> Result<Option<ArchModuleRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, label, file_count, dominant_layer, centroid_path
             FROM arch_modules WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map([module_id], |r| {
            Ok(ArchModuleRecord {
                id: r.get(0)?,
                label: r.get(1)?,
                file_count: r.get(2)?,
                dominant_layer: r.get(3)?,
                centroid_path: r.get(4)?,
            })
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// Return every arch_modules row, ordered by id. Used by the `code_map`
    /// MCP tool to render the L0 architectural overview.
    pub fn list_arch_modules(&self) -> Result<Vec<ArchModuleRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, label, file_count, dominant_layer, centroid_path
             FROM arch_modules ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ArchModuleRecord {
                id: r.get(0)?,
                label: r.get(1)?,
                file_count: r.get(2)?,
                dominant_layer: r.get(3)?,
                centroid_path: r.get(4)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Layer histogram across all arch_facts. Used by the `code_map` MCP tool.
    pub fn arch_layer_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT layer, COUNT(*) FROM arch_facts GROUP BY layer ORDER BY 2 DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Role histogram across all arch_facts.
    pub fn arch_role_counts(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT role, COUNT(*) FROM arch_facts GROUP BY role ORDER BY 2 DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Top-N files by centrality (descending). Joined with `files.path` so
    /// the MCP envelope can show paths, not ids.
    pub fn arch_top_centrality(&self, limit: usize) -> Result<Vec<(String, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.path, af.centrality
             FROM arch_facts af JOIN files f ON f.id = af.file_id
             ORDER BY af.centrality DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

/// Map a `parser::detect_language` token to its resolution family.
///
/// The cross-file resolver tiers (L2/L3/L4/L4b/L5) use this column to ensure
/// a same-name match across the symbols table is also a same-language match
/// — otherwise a Rust `target()` call gets claimed by a JS `function target()`
/// that happens to live in the repo (dogfood-found, 2026-06-17).
///
/// Families are coarser than the tree-sitter language token because real
/// projects mix dialects in one source tree:
///
/// - `ts`  — TypeScript / TSX / JavaScript / JSX (and the `.m/cjs` flavors).
///   A `.ts` importing from `.js` is the norm; they must resolve to each other.
/// - `c`   — C and C++ share a translation model close enough that headers
///   and `extern "C"` decls cross the boundary routinely.
/// - everything else is one language → one family.
///
/// Unknown / non-code languages (json, toml, yaml, md, css, html) collapse to
/// the empty string — they don't extract refs anyway, but the empty family is
/// also non-matchable, so they're invisible to the resolver tiers.
pub fn language_family(language: Option<&str>) -> &'static str {
    match language {
        Some("rust") => "rust",
        Some("go") => "go",
        Some("python") => "python",
        Some("typescript") | Some("tsx") | Some("javascript") => "ts",
        Some("c") | Some("cpp") => "c",
        Some("java") => "java",
        Some("bash") => "bash",
        _ => "",
    }
}

/// Longest common forward-slash prefix of a slice of paths. Returns "" when
/// there's nothing to share. Used as the centroid path of an arch module.
fn longest_common_prefix(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let first = &paths[0];
    let mut end = first.len();
    for p in &paths[1..] {
        let common: usize = first
            .as_bytes()
            .iter()
            .zip(p.as_bytes().iter())
            .take_while(|(a, b)| a == b)
            .count();
        if common < end {
            end = common;
        }
        if end == 0 {
            break;
        }
    }
    // Don't end on a partial path segment — back up to the last '/'.
    let trimmed = &first[..end];
    match trimmed.rfind('/') {
        Some(slash) => first[..=slash].to_string(),
        None => trimmed.to_string(),
    }
}

/// Directory prefixes that say "this is the layout convention", not "this is
/// what the module is about". Monorepos pile members under `packages/` or
/// `crates/` and a cluster that spans several of those subdirs ends up with
/// LCP `packages/` — labelling it "packages" tells the agent nothing. When
/// the centroid is one of these, we recurse one level deeper for a modal
/// segment instead.
const BORING_DIR_PREFIXES: &[&str] = &[
    "packages", "crates", "libs", "src", "lib", "app", "apps", "services", "modules", "internal",
];

fn is_boring_dir(seg: &str) -> bool {
    BORING_DIR_PREFIXES.contains(&seg)
}

/// Human-readable module label. Strategy:
///
/// 1. Centroid is a clean directory (`src/auth/`) → use the deepest segment
///    (`auth`).
/// 2. Centroid is empty OR just a boring monorepo prefix (`packages/`,
///    `crates/`, etc.) OR a partial segment that never reached a directory
///    boundary (`p`, `pa`, …) → look one level deeper. Skip the topmost
///    segment when it's also a boring prefix so a `src/graph/...` module
///    labels as "graph", not "src".
/// 3. Modal candidate must be a strict majority (>50% of total weight).
///    Below that the cluster is cross-cutting — render `cluster-{id}` so
///    the agent sees an honest "we couldn't name this" instead of a
///    misleading single segment.
/// 4. Absolutely nothing to go on → `unlabelled-{id}` (friendlier than
///    `module-{id}`, which used to leak the internal community ID).
///
/// Modal-segment counting is centrality-weighted: each member's vote is its
/// arch_facts.centrality, not 1.0. FastAPI v0.5.0 dogfood: a community
/// containing fastapi/applications.py (centrality ~0.4) was dominated by
/// ~150 docs_src/tutorial*.py files (centrality ~0.001 each) and labelled
/// "docs_src" despite applications.py being the obvious centerpiece. With
/// weights, applications.py's 0.4 beats the tutorial files' summed ~0.15
/// and the label becomes "fastapi". When centralities aren't populated
/// (every member at 0.0), we fall back to raw count so we never drop into
/// "cluster-{id}" purely from missing weights.
fn derive_label(centroid: &str, paths: &[(String, f64)], id: i64) -> String {
    // First choice: the deepest directory segment of the longest common prefix.
    // Example: members all under `src/auth/` → label "auth".
    let trimmed = centroid.trim_end_matches('/');
    let centroid_ends_in_slash = centroid.ends_with('/');
    if !trimmed.is_empty() && centroid_ends_in_slash {
        let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
        if !last.is_empty() && !is_boring_dir(last) {
            return last.to_string();
        }
    }

    // Fallback: members are scattered (no common prefix, or only a boring
    // monorepo prefix). Pick the modal directory segment across members so
    // a module of {src/graph/extractor.rs, src/graph/languages/go.rs, …}
    // labels as "graph" instead of "module-5". We always skip the topmost
    // segment when it's a known boring prefix — that's the layout, not the
    // meaning — and prefer the deepest segment we can find that isn't also
    // a boring prefix.
    use std::collections::HashMap;
    let mut weighted: HashMap<String, f64> = HashMap::new();
    let mut weighted_total: f64 = 0.0;
    let mut raw_counts: HashMap<String, usize> = HashMap::new();
    let mut raw_total: usize = 0;
    for (p, centrality) in paths {
        let segs: Vec<&str> = p.split('/').collect();
        if segs.len() < 2 {
            continue;
        }
        let pick = pick_module_segment(&segs);
        if let Some(s) = pick
            && !s.is_empty()
        {
            // Centrality is non-negative; clamp to be safe against any
            // upstream weirdness. Adding 0 to the weighted bucket is a
            // no-op so files with zero centrality just don't vote in the
            // weighted tally — the raw-count fallback below catches them.
            let w = centrality.max(0.0);
            *weighted.entry(s.clone()).or_insert(0.0) += w;
            weighted_total += w;
            *raw_counts.entry(s).or_insert(0) += 1;
            raw_total += 1;
        }
    }

    // Prefer centrality-weighted modal when any non-zero weight exists;
    // otherwise fall back to raw count so old indexes (or modules where
    // every member happens to be a leaf with centrality 0) still label.
    if weighted_total > 0.0
        && let Some((seg, w)) = weighted
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        // Strict-majority gate: top segment must own >50% of weight.
        // Below that the community is cross-cutting (or its weights are
        // spread thin enough that no single segment "owns" it) — emit
        // `cluster-{id}` so the agent sees an honest unnamed cluster.
        if *w * 2.0 > weighted_total {
            return seg.clone();
        }
        return format!("cluster-{id}");
    }
    if let Some((seg, n)) = raw_counts.iter().max_by_key(|(_, n)| **n) {
        if *n * 2 > raw_total {
            return seg.clone();
        }
        return format!("cluster-{id}");
    }
    format!("unlabelled-{id}")
}

/// Picks the modal-segment candidate for a member path: walks past boring
/// prefixes (`packages/`, `src/`, `crates/`) and returns the next segment
/// that's a real directory name. Returns `None` when we run out.
fn pick_module_segment(segs: &[&str]) -> Option<String> {
    // segs[len-1] is the file name; ignore it.
    let dirs = &segs[..segs.len().saturating_sub(1)];
    for &s in dirs {
        if s.is_empty() || is_boring_dir(s) {
            continue;
        }
        return Some(s.to_string());
    }
    None
}

#[cfg(test)]
mod arch_tests {
    use super::*;

    /// Convenience: wrap a path list with neutral centrality weights so
    /// existing tests stay readable. A constant weight of 1.0 reduces the
    /// centrality-weighted modal to the same answer as raw count.
    fn weighted(paths: &[&str]) -> Vec<(String, f64)> {
        paths.iter().map(|p| (p.to_string(), 1.0)).collect()
    }

    #[test]
    fn longest_common_prefix_basic() {
        let paths = vec![
            "src/auth/login.go".to_string(),
            "src/auth/session.go".to_string(),
        ];
        assert_eq!(longest_common_prefix(&paths), "src/auth/");
    }

    #[test]
    fn longest_common_prefix_no_overlap() {
        let paths = vec!["src/a.go".to_string(), "lib/b.go".to_string()];
        assert_eq!(longest_common_prefix(&paths), "");
    }

    #[test]
    fn longest_common_prefix_empty() {
        assert_eq!(longest_common_prefix(&[]), "");
    }

    #[test]
    fn derive_label_from_centroid() {
        // Common prefix beyond "src/" — deepest dir wins.
        assert_eq!(derive_label("src/auth/", &[], 0), "auth");
        // No common prefix AND no paths — fall through to "unlabelled-N".
        assert_eq!(derive_label("", &[], 7), "unlabelled-7");
    }

    #[test]
    fn derive_label_modal_second_segment() {
        // Scattered members under src/graph/* — modal 2nd segment "graph" wins.
        let paths = weighted(&[
            "src/graph/extractor.rs",
            "src/graph/languages/go.rs",
            "src/graph/languages/rust_lang.rs",
        ]);
        assert_eq!(derive_label("", &paths, 5), "graph");
    }

    #[test]
    fn derive_label_skips_src_in_common_prefix() {
        // Common prefix is just "src/" — that's the boring top-level.
        // Should fall through to modal 2nd segment, which is "storage".
        let paths = weighted(&["src/storage/repo.rs", "src/storage/schema.rs"]);
        assert_eq!(derive_label("src/", &paths, 0), "storage");
    }

    #[test]
    fn derive_label_packages_centroid_recurses_into_package_name() {
        // Vite-style: members all under `packages/vite/...`. Pre-fix this
        // labelled the cluster "packages" (or "p-N" via collision); now we
        // recurse past the boring `packages/` prefix and use "vite".
        let paths = weighted(&[
            "packages/vite/src/node/server/index.ts",
            "packages/vite/src/node/server/middlewares.ts",
            "packages/vite/src/node/utils.ts",
        ]);
        assert_eq!(derive_label("packages/vite/src/", &paths, 0), "vite");
        // And when the centroid only reaches `packages/` (members straddle
        // two sub-packages but most are still in vite), the modal-segment
        // fallback resolves to "vite" too.
        assert_eq!(derive_label("packages/", &paths, 0), "vite");
    }

    #[test]
    fn derive_label_crates_centroid_recurses_into_crate_name() {
        // Helix-style: members under `crates/helix-view/...`.
        let paths = weighted(&[
            "crates/helix-view/src/editor.rs",
            "crates/helix-view/src/view.rs",
            "crates/helix-view/src/document.rs",
        ]);
        assert_eq!(derive_label("crates/helix-view/", &paths, 0), "helix-view");
        assert_eq!(derive_label("crates/", &paths, 0), "helix-view");
    }

    #[test]
    fn derive_label_partial_segment_centroid_falls_through_to_modal() {
        // A cluster spanning `packages/` AND `playground/` has LCP "p" — a
        // partial segment, not even a full directory. Pre-fix this rendered
        // as "p" and collided with siblings to become "p-N"; now we ignore
        // the partial centroid and recurse into the modal segment.
        let paths = weighted(&[
            "packages/vite/src/node/cli.ts",
            "packages/vite/src/node/config.ts",
            "playground/vue/main.ts",
        ]);
        assert_eq!(derive_label("p", &paths, 0), "vite");
    }

    #[test]
    fn derive_label_cross_cutting_cluster_renders_as_cluster() {
        // No single segment owns >50% of the members — old behaviour was
        // `mixed:{seg}+` which leaked internal merge state. New honest name:
        // `cluster-{id}`.
        let paths = weighted(&[
            "src/storage/repo.rs",
            "src/search/symbol.rs",
            "src/temporal/git_parser.rs",
            "src/temporal/hotspot.rs",
            "src/indexer/pipeline.rs",
            "src/indexer/walker.rs",
        ]);
        // Six members across four distinct second segments — no majority.
        assert_eq!(derive_label("src/", &paths, 42), "cluster-42");
    }

    #[test]
    fn derive_label_no_data_returns_unlabelled() {
        // Empty centroid + zero paths — friendlier than the old
        // `module-{id}` which sounded like a real name.
        assert_eq!(derive_label("", &[], 9), "unlabelled-9");
    }

    #[test]
    fn derive_label_centrality_weighted_modal_beats_raw_count() {
        // FastAPI v0.5.0 dogfood shape: a community containing
        // fastapi/applications.py (high centrality) plus a bulk of
        // docs_src/tutorial*.py files (each near zero) was labelled
        // "docs_src" pre-fix because raw count put it at 10/12 = 83%.
        // Centrality-weighted: applications.py + routing.py contribute
        // 0.4+0.3=0.7, all 10 tutorial files contribute 0.001×10=0.01,
        // so fastapi wins with 0.7/0.71 = 98.5%.
        let mut paths: Vec<(String, f64)> = (1..=10)
            .map(|i| (format!("docs_src/tutorial{i:03}.py"), 0.001))
            .collect();
        paths.push(("src/fastapi/applications.py".into(), 0.4));
        paths.push(("src/fastapi/routing.py".into(), 0.3));
        assert_eq!(derive_label("", &paths, 0), "fastapi");
    }

    #[test]
    fn derive_label_zero_centrality_falls_back_to_raw_count() {
        // When every member has centrality 0 (a leaf-only community, or
        // an old index that didn't populate the column), the weighted
        // bucket sums to 0 and we'd otherwise drop into cluster-{id}.
        // The raw-count fallback keeps the old behaviour intact so the
        // change is strictly additive when centrality data is missing.
        let paths: Vec<(String, f64)> = vec![
            ("src/storage/repo.rs".into(), 0.0),
            ("src/storage/schema.rs".into(), 0.0),
            ("src/storage/cache.rs".into(), 0.0),
        ];
        assert_eq!(derive_label("src/", &paths, 0), "storage");
    }

    #[test]
    fn derive_label_centrality_weighted_no_majority_renders_as_cluster() {
        // Two segments split centrality near-evenly — even with weighting
        // the >50% gate refuses to pick a winner and the honest cluster
        // tag wins.
        let paths: Vec<(String, f64)> = vec![
            ("src/storage/repo.rs".into(), 0.4),
            ("src/storage/schema.rs".into(), 0.3),
            ("src/search/symbol.rs".into(), 0.4),
            ("src/search/fulltext.rs".into(), 0.3),
        ];
        assert_eq!(derive_label("src/", &paths, 7), "cluster-7");
    }
}
