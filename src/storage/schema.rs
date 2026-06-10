use anyhow::Result;
use rusqlite::Connection;

pub const SCHEMA_VERSION: u32 = 3;

pub fn init_db(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    conn.execute_batch("PRAGMA cache_size = -64000;")?; // 64MB cache

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS files (
            id       INTEGER PRIMARY KEY,
            path     TEXT NOT NULL UNIQUE,
            hash     TEXT NOT NULL,
            language TEXT,
            mtime    INTEGER NOT NULL,
            size     INTEGER NOT NULL,
            dir      TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
        CREATE INDEX IF NOT EXISTS idx_files_dir ON files(dir);

        CREATE TABLE IF NOT EXISTS chunks (
            id         INTEGER PRIMARY KEY,
            file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            content    TEXT NOT NULL,
            kind       TEXT NOT NULL,  -- function, class, module, block, comment
            start_line INTEGER NOT NULL,
            end_line   INTEGER NOT NULL,
            scope      TEXT,           -- parent scope chain, e.g. 'mod::Struct::method'
            token_count INTEGER NOT NULL,
            UNIQUE(file_id, start_line, end_line)
        );
        CREATE INDEX IF NOT EXISTS idx_chunks_file ON chunks(file_id);

        CREATE TABLE IF NOT EXISTS symbols (
            id       INTEGER PRIMARY KEY,
            chunk_id INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
            file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            name     TEXT NOT NULL,
            kind     TEXT NOT NULL,  -- function, class, struct, enum, const, variable, import
            line     INTEGER NOT NULL,
            scope    TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
        CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
        CREATE INDEX IF NOT EXISTS idx_symbols_name_file ON symbols(name, file_id);

        -- FTS5 full-text index on chunk content
        CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
            content,
            scope,
            content='chunks',
            content_rowid='id',
            tokenize='unicode61 remove_diacritics 2'
        );

        -- Triggers to keep FTS5 in sync
        CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
            INSERT INTO chunks_fts(rowid, content, scope)
            VALUES (new.id, new.content, new.scope);
        END;
        CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
            INSERT INTO chunks_fts(chunks_fts, rowid, content, scope)
            VALUES ('delete', old.id, old.content, old.scope);
        END;
        CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
            INSERT INTO chunks_fts(chunks_fts, rowid, content, scope)
            VALUES ('delete', old.id, old.content, old.scope);
            INSERT INTO chunks_fts(rowid, content, scope)
            VALUES (new.id, new.content, new.scope);
        END;

        -- ═══ v0.2: Code Graph ═══

        CREATE TABLE IF NOT EXISTS refs (
            id              INTEGER PRIMARY KEY,
            source_file_id  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
            source_line     INTEGER NOT NULL,
            source_symbol   TEXT,
            target_name     TEXT NOT NULL,
            target_qualifier TEXT,
            receiver_type   TEXT,   -- v3: inferred type of the call receiver (x.M() where x: T)
            target_file_id  INTEGER REFERENCES files(id) ON DELETE SET NULL,
            target_symbol_id INTEGER REFERENCES symbols(id) ON DELETE SET NULL,
            kind            TEXT NOT NULL,
            confidence      REAL DEFAULT 0.0
        );
        CREATE INDEX IF NOT EXISTS idx_refs_source ON refs(source_file_id);
        CREATE INDEX IF NOT EXISTS idx_refs_target_file ON refs(target_file_id);
        CREATE INDEX IF NOT EXISTS idx_refs_target_name ON refs(target_name);
        CREATE INDEX IF NOT EXISTS idx_refs_kind ON refs(kind);

        -- ═══ v0.2: Temporal ═══

        CREATE TABLE IF NOT EXISTS commits (
            id      INTEGER PRIMARY KEY,
            hash    TEXT NOT NULL UNIQUE,
            author  TEXT NOT NULL,
            ts      INTEGER NOT NULL,
            message TEXT
        );

        CREATE TABLE IF NOT EXISTS commit_files (
            commit_id INTEGER NOT NULL REFERENCES commits(id) ON DELETE CASCADE,
            file_path TEXT NOT NULL,
            added     INTEGER DEFAULT 0,
            deleted   INTEGER DEFAULT 0,
            PRIMARY KEY (commit_id, file_path)
        );
        CREATE INDEX IF NOT EXISTS idx_cf_path ON commit_files(file_path);

        -- ═══ v0.2: Precomputed Metrics ═══

        CREATE TABLE IF NOT EXISTS file_metrics (
            file_id          INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
            cyclomatic        REAL DEFAULT 0,
            max_func_lines    INTEGER DEFAULT 0,
            change_count_30d  INTEGER DEFAULT 0,
            change_count_90d  INTEGER DEFAULT 0,
            last_changed_ts   INTEGER,
            unique_authors    INTEGER DEFAULT 0,
            hotspot_score     REAL DEFAULT 0,
            has_tests         INTEGER DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS change_coupling (
            file_a         TEXT NOT NULL,
            file_b         TEXT NOT NULL,
            co_changes     INTEGER NOT NULL,
            total_changes  INTEGER NOT NULL,
            coupling_score REAL NOT NULL,
            PRIMARY KEY (file_a, file_b)
        );
        ",
    )?;

    // v2 → v3 migration: CREATE IF NOT EXISTS won't add columns to existing
    // tables, so ALTER explicitly. Idempotent — duplicate-column error means
    // the column is already there.
    let version: u32 = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if version < 3 {
        let _ = conn.execute("ALTER TABLE refs ADD COLUMN receiver_type TEXT", []);
    }

    // Set schema version
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES ('schema_version', ?1)",
        [SCHEMA_VERSION.to_string()],
    )?;

    Ok(())
}

pub fn init_vec_table(conn: &Connection, dimensions: usize) -> Result<()> {
    conn.execute_batch(&format!(
        "
        CREATE VIRTUAL TABLE IF NOT EXISTS vec_chunks USING vec0(
            chunk_id INTEGER PRIMARY KEY,
            embedding float[{dimensions}]
        );
        "
    ))?;
    Ok(())
}
