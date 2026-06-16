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
        self.conn.execute(
            "INSERT INTO files(path, hash, language, mtime, size, dir, generated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET hash=?2, language=?3, mtime=?4, size=?5, dir=?6, generated=?7",
            params![path, hash, language, mtime, size, dir, generated as i64],
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

    pub fn find_callers_of(
        &self,
        target_name: &str,
        target_file_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<RefRecord>> {
        let mut stmt = if let Some(fid) = target_file_id {
            let mut s = self.conn.prepare(
                "SELECT r.id, r.source_file_id, r.source_line, r.source_symbol,
                        r.target_name, r.kind, r.confidence, f.path
                 FROM refs r JOIN files f ON r.source_file_id = f.id
                 WHERE r.target_name = ?1 AND (r.target_file_id = ?2 OR r.target_file_id IS NULL)
                 AND r.kind IN ('call','field_access')
                 ORDER BY r.confidence DESC LIMIT ?3",
            )?;
            return Ok(s
                .query_map(params![target_name, fid, limit as i64], |row| {
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
                })?
                .filter_map(|r| r.ok())
                .collect());
        } else {
            self.conn.prepare(
                "SELECT r.id, r.source_file_id, r.source_line, r.source_symbol,
                        r.target_name, r.kind, r.confidence, f.path
                 FROM refs r JOIN files f ON r.source_file_id = f.id
                 WHERE r.target_name = ?1
                 AND r.kind IN ('call','field_access')
                 ORDER BY r.confidence DESC LIMIT ?2",
            )?
        };
        let rows = stmt.query_map(params![target_name, limit as i64], |row| {
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
        // Pull paths for every module member in one query, so module labels can
        // be the longest common prefix of their member files. The inference
        // engine left `label` / `centroid_path` empty; we fill them here using
        // the freshly-written arch_facts.module_id → file_id mapping.
        let mut member_paths: std::collections::HashMap<i64, Vec<String>> =
            std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT af.module_id, f.path
                 FROM arch_facts af JOIN files f ON f.id = af.file_id
                 WHERE af.module_id >= 0",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for r in rows.flatten() {
                member_paths.entry(r.0).or_default().push(r.1);
            }
        }

        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM arch_modules", [])?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO arch_modules(id, label, file_count, dominant_layer, centroid_path)
                 VALUES (?1,?2,?3,?4,?5)",
            )?;
            for m in modules {
                let paths = member_paths.get(&m.id).cloned().unwrap_or_default();
                let centroid_path = longest_common_prefix(&paths);
                let label = derive_label(&centroid_path, &paths, m.id);
                stmt.execute(params![
                    m.id,
                    label,
                    m.file_count,
                    m.dominant_layer,
                    centroid_path,
                ])?;
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

/// Human-readable module label. Prefers the deepest meaningful directory name
/// from the centroid path; falls back to `module-{id}` when there's nothing
/// to derive.
fn derive_label(centroid: &str, _paths: &[String], id: i64) -> String {
    let trimmed = centroid.trim_end_matches('/');
    if trimmed.is_empty() {
        return format!("module-{id}");
    }
    let last = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if last.is_empty() {
        format!("module-{id}")
    } else {
        last.to_string()
    }
}

#[cfg(test)]
mod arch_tests {
    use super::*;

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
        assert_eq!(derive_label("src/auth/", &[], 0), "auth");
        assert_eq!(derive_label("", &[], 7), "module-7");
    }
}
