use anyhow::Result;
use rusqlite::Connection;

pub const SCHEMA_VERSION: u32 = 6;

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
            dir      TEXT NOT NULL DEFAULT '',
            generated INTEGER NOT NULL DEFAULT 0,  -- machine-generated (DO NOT EDIT); refs skipped
            lang_family TEXT NOT NULL DEFAULT ''   -- v6: denormalized language family for the cross-file resolver tiers (ts/tsx/js collapse to 'ts', etc.)
        );
        CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
        CREATE INDEX IF NOT EXISTS idx_files_dir ON files(dir);
        CREATE INDEX IF NOT EXISTS idx_files_lang_family ON files(lang_family);

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
        -- Binding-tier lookups: (source_file, name) → binding, correlated per
        -- unresolved ref; partial index keeps it tiny.
        CREATE INDEX IF NOT EXISTS idx_refs_binding ON refs(source_file_id, target_name)
            WHERE kind = 'import_binding';

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

        -- ═══ v5: Architectural coordinates (L0 inference) ═══
        --
        -- One row per indexed file: layer + role + module assignment + topology
        -- evidence. Replaced wholesale at the end of each indexing pass.
        CREATE TABLE IF NOT EXISTS arch_facts (
            file_id           INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
            layer             TEXT    NOT NULL DEFAULT 'unknown',
            role              TEXT    NOT NULL DEFAULT 'leaf',
            module_id         INTEGER NOT NULL DEFAULT -1,
            depth_from_entry  INTEGER,              -- NULL when unreachable
            centrality        REAL    NOT NULL DEFAULT 0,
            in_degree         INTEGER NOT NULL DEFAULT 0,
            out_degree        INTEGER NOT NULL DEFAULT 0,
            layer_conf        REAL    NOT NULL DEFAULT 0,
            signals           TEXT    NOT NULL DEFAULT '{}'  -- JSON debug envelope
        );
        CREATE INDEX IF NOT EXISTS idx_arch_facts_layer ON arch_facts(layer);
        CREATE INDEX IF NOT EXISTS idx_arch_facts_role ON arch_facts(role);
        CREATE INDEX IF NOT EXISTS idx_arch_facts_module ON arch_facts(module_id);

        -- One row per detected Louvain community. label is a human-readable
        -- best-effort name (longest common dir prefix of members) so callers
        -- can show auth instead of module-7.
        CREATE TABLE IF NOT EXISTS arch_modules (
            id              INTEGER PRIMARY KEY,
            label           TEXT NOT NULL,
            file_count      INTEGER NOT NULL DEFAULT 0,
            dominant_layer  TEXT NOT NULL DEFAULT 'unknown',
            centroid_path   TEXT NOT NULL DEFAULT ''
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
    if version < 4 {
        let _ = conn.execute(
            "ALTER TABLE files ADD COLUMN generated INTEGER NOT NULL DEFAULT 0",
            [],
        );
    }
    if version < 5 {
        // arch_facts / arch_modules are created via the CREATE IF NOT EXISTS
        // batch above; no ALTER needed. We bump the recorded version so
        // future migrations know this snapshot already has the L0 tables.
    }
    if version < 6 {
        // v6: denormalized language family on `files` so the cross-file
        // resolver tiers (L2/L3/L4/L4b/L5) can filter same-family with a
        // single equality JOIN instead of nested SELECTs. ALTER is
        // idempotent — duplicate-column error means the column is already
        // there (existing rows get the column DEFAULT '' and stay
        // unfiltered until a reindex repopulates them).
        let _ = conn.execute(
            "ALTER TABLE files ADD COLUMN lang_family TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_files_lang_family ON files(lang_family)",
            [],
        );
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
