use anyhow::Result;
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::compression;

/// Computes the SHA-256 hash of data, returning the hex string.
pub fn hash_data(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Stores a blob if it doesn't already exist (content-addressable dedup).
/// Returns the hash of the data.
pub fn store(
    conn: &Connection,
    data: &[u8],
    config: &compression::Config,
) -> Result<String> {
    let hash = hash_data(data);

    // Check if blob already exists (deduplication)
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM blobs WHERE hash = ?1",
        [&hash],
        |row| row.get(0),
    )?;

    if !exists {
        let compressed = compression::compress(data, config)?;
        conn.execute(
            "INSERT INTO blobs (hash, data, size_raw, size_stored, compression) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                hash,
                compressed,
                data.len() as i64,
                compressed.len() as i64,
                config.algorithm.as_str(),
            ],
        )?;
    }

    Ok(hash)
}

/// Retrieves and decompresses a blob by hash.
pub fn retrieve(conn: &Connection, hash: &str) -> Result<Vec<u8>> {
    let (data, algo): (Vec<u8>, String) = conn.query_row(
        "SELECT data, compression FROM blobs WHERE hash = ?1",
        [hash],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    compression::decompress(&data, &algo)
}

/// Returns blob metadata (size_raw, size_stored, compression) without reading data.
pub fn info(conn: &Connection, hash: &str) -> Result<(i64, i64, String)> {
    let result = conn.query_row(
        "SELECT size_raw, size_stored, compression FROM blobs WHERE hash = ?1",
        [hash],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(result)
}

/// Checks if a blob with the given hash exists.
pub fn exists(conn: &Connection, hash: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM blobs WHERE hash = ?1",
        [hash],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blobs (
                hash TEXT PRIMARY KEY,
                data BLOB NOT NULL,
                size_raw INTEGER NOT NULL,
                size_stored INTEGER NOT NULL,
                compression TEXT NOT NULL DEFAULT 'zstd'
            );"
        ).unwrap();
        conn
    }

    #[test]
    fn test_store_and_retrieve() {
        let conn = setup();
        let data = b"Hello, blob storage!";
        let config = compression::Config::default();

        let hash = store(&conn, data, &config).unwrap();
        let retrieved = retrieve(&conn, &hash).unwrap();
        assert_eq!(retrieved, data);
    }

    #[test]
    fn test_deduplication() {
        let conn = setup();
        let data = b"Duplicate content";
        let config = compression::Config::default();

        let hash1 = store(&conn, data, &config).unwrap();
        let hash2 = store(&conn, data, &config).unwrap();
        assert_eq!(hash1, hash2);

        // Only one blob should be stored
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_blob_info() {
        let conn = setup();
        let data = b"Info test data";
        let config = compression::Config::default();

        let hash = store(&conn, data, &config).unwrap();
        let (raw, stored, algo) = info(&conn, &hash).unwrap();
        assert_eq!(raw, data.len() as i64);
        assert!(stored > 0);
        assert_eq!(algo, "zstd");
    }

    #[test]
    fn test_exists() {
        let conn = setup();
        let data = b"Existence test";
        let config = compression::Config::default();

        let hash = store(&conn, data, &config).unwrap();
        assert!(exists(&conn, &hash).unwrap());
        assert!(!exists(&conn, "nonexistent_hash").unwrap());
    }

    #[test]
    fn test_hash_deterministic() {
        let data = b"deterministic hash test";
        let h1 = hash_data(data);
        let h2 = hash_data(data);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex is 64 chars
    }
}
