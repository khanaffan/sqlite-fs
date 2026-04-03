use anyhow::Result;
use rusqlite::Connection;

const SCHEMA: &str = r#"
-- Snapshots represent a point-in-time backup
CREATE TABLE IF NOT EXISTS snapshots (
    id          INTEGER PRIMARY KEY,
    label       TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    source_path TEXT NOT NULL,
    notes       TEXT
);

-- Nodes represent files and directories
CREATE TABLE IF NOT EXISTS nodes (
    id          INTEGER PRIMARY KEY,
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    parent_id   INTEGER REFERENCES nodes(id),
    name        TEXT NOT NULL,
    type        TEXT NOT NULL CHECK (type IN ('file', 'directory', 'symlink')),
    size        INTEGER DEFAULT 0,
    mode        INTEGER,
    owner       TEXT,
    grp         TEXT,
    created_at  TEXT,
    modified_at TEXT,
    accessed_at TEXT,
    blob_hash   TEXT,
    extra       TEXT
);

-- Content-addressable blob storage with deduplication
CREATE TABLE IF NOT EXISTS blobs (
    hash        TEXT PRIMARY KEY,
    data        BLOB NOT NULL,
    size_raw    INTEGER NOT NULL,
    size_stored INTEGER NOT NULL,
    compression TEXT NOT NULL DEFAULT 'zstd'
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_nodes_snapshot ON nodes(snapshot_id);
CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id);
CREATE INDEX IF NOT EXISTS idx_nodes_hash ON nodes(blob_hash);
CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);
"#;

pub fn open_or_create(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_readonly(path: &str) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();

        // Verify tables exist
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('snapshots','nodes','blobs')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }
}
