use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug)]
pub struct SnapshotInfo {
    pub id: i64,
    pub label: Option<String>,
    pub created_at: String,
    pub source_path: String,
    pub notes: Option<String>,
    pub file_count: i64,
    pub dir_count: i64,
    pub total_size: i64,
}

#[derive(Debug)]
pub struct FileEntry {
    pub name: String,
    pub node_type: String,
    pub size: i64,
    pub modified_at: Option<String>,
    pub blob_hash: Option<String>,
}

/// List all snapshots in the archive.
pub fn list_snapshots(conn: &Connection) -> Result<Vec<SnapshotInfo>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.label, s.created_at, s.source_path, s.notes,
                (SELECT COUNT(*) FROM nodes n WHERE n.snapshot_id = s.id AND n.type = 'file'),
                (SELECT COUNT(*) FROM nodes n WHERE n.snapshot_id = s.id AND n.type = 'directory'),
                (SELECT COALESCE(SUM(n.size), 0) FROM nodes n WHERE n.snapshot_id = s.id AND n.type = 'file')
         FROM snapshots s ORDER BY s.id"
    )?;

    let snapshots = stmt
        .query_map([], |row| {
            Ok(SnapshotInfo {
                id: row.get(0)?,
                label: row.get(1)?,
                created_at: row.get(2)?,
                source_path: row.get(3)?,
                notes: row.get(4)?,
                file_count: row.get(5)?,
                dir_count: row.get(6)?,
                total_size: row.get(7)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(snapshots)
}

/// List files in a snapshot, optionally under a specific path.
pub fn list_files(conn: &Connection, snapshot_id: i64, parent_id: Option<i64>) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    if let Some(pid) = parent_id {
        let mut stmt = conn.prepare(
            "SELECT name, type, size, modified_at, blob_hash FROM nodes WHERE snapshot_id = ?1 AND parent_id = ?2 ORDER BY type DESC, name"
        )?;
        let rows = stmt.query_map(rusqlite::params![snapshot_id, pid], |row| {
            Ok(FileEntry {
                name: row.get(0)?,
                node_type: row.get(1)?,
                size: row.get(2)?,
                modified_at: row.get(3)?,
                blob_hash: row.get(4)?,
            })
        })?;
        for row in rows {
            entries.push(row?);
        }
    } else {
        // List root-level entries (where parent_id is NULL or root node children)
        let mut stmt = conn.prepare(
            "SELECT name, type, size, modified_at, blob_hash FROM nodes WHERE snapshot_id = ?1 AND parent_id IS NULL ORDER BY type DESC, name"
        )?;
        let rows = stmt.query_map([snapshot_id], |row| {
            Ok(FileEntry {
                name: row.get(0)?,
                node_type: row.get(1)?,
                size: row.get(2)?,
                modified_at: row.get(3)?,
                blob_hash: row.get(4)?,
            })
        })?;
        for row in rows {
            entries.push(row?);
        }

        // If root has one directory entry ".", list its children instead
        if entries.len() == 1 && entries[0].name == "." && entries[0].node_type == "directory" {
            let root_id: i64 = conn.query_row(
                "SELECT id FROM nodes WHERE snapshot_id = ?1 AND parent_id IS NULL AND name = '.' LIMIT 1",
                [snapshot_id],
                |row| row.get(0),
            )?;
            return list_files(conn, snapshot_id, Some(root_id));
        }
    }

    Ok(entries)
}

/// Find files by name pattern across all snapshots.
pub fn find_files(conn: &Connection, pattern: &str) -> Result<Vec<(i64, String, i64)>> {
    let sql_pattern = pattern.replace('*', "%").replace('?', "_");
    let mut stmt = conn.prepare(
        "SELECT snapshot_id, name, size FROM nodes WHERE name LIKE ?1 AND type = 'file' ORDER BY snapshot_id, name"
    )?;
    let results: Vec<(i64, String, i64)> = stmt
        .query_map([&sql_pattern], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(results)
}

/// Find files larger than a given size.
pub fn find_larger_than(conn: &Connection, snapshot_id: i64, min_size: i64) -> Result<Vec<FileEntry>> {
    let mut stmt = conn.prepare(
        "SELECT name, type, size, modified_at, blob_hash FROM nodes WHERE snapshot_id = ?1 AND type = 'file' AND size > ?2 ORDER BY size DESC"
    )?;
    let results = stmt
        .query_map(rusqlite::params![snapshot_id, min_size], |row| {
            Ok(FileEntry {
                name: row.get(0)?,
                node_type: row.get(1)?,
                size: row.get(2)?,
                modified_at: row.get(3)?,
                blob_hash: row.get(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(results)
}

/// Show the history of a file across snapshots.
pub fn file_history(conn: &Connection, file_name: &str) -> Result<Vec<(i64, String, i64, Option<String>)>> {
    let name = std::path::Path::new(file_name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());

    let mut stmt = conn.prepare(
        "SELECT n.snapshot_id, s.created_at, n.size, n.blob_hash
         FROM nodes n JOIN snapshots s ON n.snapshot_id = s.id
         WHERE n.name = ?1 AND n.type = 'file'
         ORDER BY n.snapshot_id"
    )?;
    let results = stmt
        .query_map([&name], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(results)
}

/// Get archive statistics.
pub struct ArchiveInfo {
    pub snapshot_count: i64,
    pub total_files: i64,
    pub total_dirs: i64,
    pub total_blobs: i64,
    pub total_raw_bytes: i64,
    pub total_stored_bytes: i64,
    pub db_size: i64,
}

pub fn archive_info(conn: &Connection, db_path: &str) -> Result<ArchiveInfo> {
    let snapshot_count: i64 = conn.query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))?;
    let total_files: i64 = conn.query_row("SELECT COUNT(*) FROM nodes WHERE type = 'file'", [], |row| row.get(0))?;
    let total_dirs: i64 = conn.query_row("SELECT COUNT(*) FROM nodes WHERE type = 'directory'", [], |row| row.get(0))?;
    let total_blobs: i64 = conn.query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))?;
    let total_raw_bytes: i64 = conn.query_row("SELECT COALESCE(SUM(size_raw), 0) FROM blobs", [], |row| row.get(0))?;
    let total_stored_bytes: i64 = conn.query_row("SELECT COALESCE(SUM(size_stored), 0) FROM blobs", [], |row| row.get(0))?;

    let db_size = std::fs::metadata(db_path).map(|m| m.len() as i64).unwrap_or(0);

    Ok(ArchiveInfo {
        snapshot_count,
        total_files,
        total_dirs,
        total_blobs,
        total_raw_bytes,
        total_stored_bytes,
        db_size,
    })
}

/// Verify archive integrity by checking all blob hashes.
pub fn verify(conn: &Connection) -> Result<(u64, u64)> {
    let mut stmt = conn.prepare("SELECT hash, data, compression FROM blobs")?;
    let blobs: Vec<(String, Vec<u8>, String)> = stmt
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut ok = 0u64;
    let mut failed = 0u64;

    for (expected_hash, data, algo) in &blobs {
        match crate::compression::decompress(data, algo) {
            Ok(decompressed) => {
                let actual_hash = crate::blob::hash_data(&decompressed);
                if actual_hash == *expected_hash {
                    ok += 1;
                } else {
                    eprintln!("CORRUPT: blob {} hash mismatch", expected_hash);
                    failed += 1;
                }
            }
            Err(e) => {
                eprintln!("CORRUPT: blob {} decompress failed: {}", expected_hash, e);
                failed += 1;
            }
        }
    }

    Ok((ok, failed))
}

/// Prune old snapshots, keeping the last N.
pub fn prune(conn: &Connection, keep_last: usize) -> Result<u64> {
    let all_ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM snapshots ORDER BY id DESC")?;
        stmt.query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect()
    };

    if all_ids.len() <= keep_last {
        return Ok(0);
    }

    let to_delete = &all_ids[keep_last..];
    let mut deleted = 0u64;

    for id in to_delete {
        conn.execute("DELETE FROM nodes WHERE snapshot_id = ?1", [id])?;
        conn.execute("DELETE FROM snapshots WHERE id = ?1", [id])?;
        deleted += 1;
    }

    // Clean up orphaned blobs
    conn.execute(
        "DELETE FROM blobs WHERE hash NOT IN (SELECT DISTINCT blob_hash FROM nodes WHERE blob_hash IS NOT NULL)",
        [],
    )?;

    Ok(deleted)
}

/// Compact the database (VACUUM).
pub fn compact(conn: &Connection) -> Result<()> {
    conn.execute_batch("VACUUM")?;
    Ok(())
}

/// Execute a raw SQL query and return results as formatted strings.
pub fn raw_query(conn: &Connection, sql: &str) -> Result<Vec<Vec<String>>> {
    let mut stmt = conn.prepare(sql)?;
    let col_count = stmt.column_count();
    let col_names: Vec<String> = (0..col_count)
        .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
        .collect();

    let mut results = vec![col_names];

    let rows = stmt.query_map([], |row| {
        let mut values = Vec::new();
        for i in 0..col_count {
            let val: String = row.get::<_, rusqlite::types::Value>(i).map(|v| match v {
                rusqlite::types::Value::Null => "NULL".to_string(),
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Real(f) => f.to_string(),
                rusqlite::types::Value::Text(s) => s,
                rusqlite::types::Value::Blob(b) => format!("<blob {} bytes>", b.len()),
            }).unwrap_or_else(|_| "?".to_string());
            values.push(val);
        }
        Ok(values)
    })?;

    for row in rows {
        results.push(row?);
    }

    Ok(results)
}
