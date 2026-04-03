use anyhow::{Result, Context};
use globset::{Glob, GlobSet, GlobSetBuilder};
use rusqlite::Connection;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use walkdir::WalkDir;
use chrono::{DateTime, Utc};
use indicatif::{ProgressBar, ProgressStyle};

use crate::{blob, compression};

pub struct BackupOptions {
    pub label: Option<String>,
    pub notes: Option<String>,
    pub compression: compression::Config,
    pub exclude: Vec<String>,
    pub incremental: bool,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            label: None,
            notes: None,
            compression: compression::Config::default(),
            exclude: Vec::new(),
            incremental: false,
        }
    }
}

pub struct BackupStats {
    pub files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub bytes_raw: u64,
    pub bytes_stored: u64,
    pub blobs_deduped: u64,
    pub snapshot_id: i64,
}

/// Perform a full or incremental backup of a source directory.
pub fn backup(conn: &Connection, source: &str, opts: &BackupOptions) -> Result<BackupStats> {
    let source_path = Path::new(source)
        .canonicalize()
        .with_context(|| format!("source path not found: {}", source))?;

    let source_str = source_path.to_string_lossy().to_string();

    let exclude_set = build_exclude_set(&opts.exclude)?;

    // Create snapshot
    let snapshot_id: i64 = {
        conn.execute(
            "INSERT INTO snapshots (label, source_path, notes) VALUES (?1, ?2, ?3)",
            rusqlite::params![opts.label, source_str, opts.notes],
        )?;
        conn.last_insert_rowid()
    };

    // Get previous snapshot for incremental mode
    let prev_snapshot_id: Option<i64> = if opts.incremental {
        conn.query_row(
            "SELECT id FROM snapshots WHERE source_path = ?1 AND id < ?2 ORDER BY id DESC LIMIT 1",
            rusqlite::params![source_str, snapshot_id],
            |row| row.get(0),
        )
        .ok()
    } else {
        None
    };

    let mut stats = BackupStats {
        files: 0,
        directories: 0,
        symlinks: 0,
        bytes_raw: 0,
        bytes_stored: 0,
        blobs_deduped: 0,
        snapshot_id,
    };

    // Count entries for progress bar
    let entries: Vec<_> = WalkDir::new(&source_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| !should_exclude(e.path(), &source_path, &exclude_set))
        .collect();

    let pb = ProgressBar::new(entries.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("█▓░"),
    );

    // Map from filesystem path to node ID for parent lookups
    let mut path_to_id: std::collections::HashMap<std::path::PathBuf, i64> = std::collections::HashMap::new();

    for entry in &entries {
        let path = entry.path();
        let rel_path = path.strip_prefix(&source_path).unwrap_or(path);
        let metadata = entry.metadata()?;

        let name = rel_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| ".".to_string());

        // Determine parent_id
        let parent_id = if rel_path == Path::new("") || rel_path == Path::new(".") {
            None
        } else {
            rel_path.parent().and_then(|p| {
                if p == Path::new("") {
                    path_to_id.get(&source_path)
                } else {
                    path_to_id.get(&source_path.join(p))
                }
            }).copied()
        };

        let (node_type, blob_hash, file_size) = if metadata.is_dir() {
            stats.directories += 1;
            ("directory".to_string(), None::<String>, 0i64)
        } else if metadata.is_symlink() {
            stats.symlinks += 1;
            let target = fs::read_link(path)?;
            let _extra = serde_json::json!({"symlink_target": target.to_string_lossy()});
            ("symlink".to_string(), None, 0i64)
        } else {
            // Regular file
            let data = fs::read(path)
                .with_context(|| format!("failed to read file: {}", path.display()))?;
            let size = data.len() as i64;
            stats.files += 1;
            stats.bytes_raw += size as u64;

            // Check incremental: skip if unchanged
            if let Some(prev_sid) = prev_snapshot_id {
                let rel_str = rel_path.to_string_lossy();
                if is_unchanged(conn, prev_sid, &rel_str, &metadata)? {
                    // Reuse previous blob hash
                    let prev_hash: Option<String> = find_previous_hash(conn, prev_sid, &rel_str)?;
                    if let Some(hash) = prev_hash {
                        stats.blobs_deduped += 1;
                        pb.inc(1);

                        let node_id = insert_node(
                            conn, snapshot_id, parent_id, &name, "file", size,
                            &metadata, Some(&hash),
                        )?;
                        path_to_id.insert(source_path.join(rel_path), node_id);
                        continue;
                    }
                }
            }

            let already_exists = {
                let hash = blob::hash_data(&data);
                blob::exists(conn, &hash)?
            };

            let hash = blob::store(conn, &data, &opts.compression)?;

            if already_exists {
                stats.blobs_deduped += 1;
            } else {
                let (_, stored, _) = blob::info(conn, &hash)?;
                stats.bytes_stored += stored as u64;
            }

            ("file".to_string(), Some(hash), size)
        };

        let node_id = insert_node(
            conn, snapshot_id, parent_id, &name, &node_type, file_size,
            &metadata, blob_hash.as_deref(),
        )?;
        path_to_id.insert(source_path.join(rel_path), node_id);

        pb.set_message(name.clone());
        pb.inc(1);
    }

    pb.finish_with_message("backup complete");
    Ok(stats)
}

fn insert_node(
    conn: &Connection,
    snapshot_id: i64,
    parent_id: Option<i64>,
    name: &str,
    node_type: &str,
    size: i64,
    metadata: &fs::Metadata,
    blob_hash: Option<&str>,
) -> Result<i64> {
    #[cfg(unix)]
    let mode = metadata.mode() as i64;
    #[cfg(not(unix))]
    let mode = 0i64;
    let modified = system_time_to_iso(metadata.modified().ok());
    let accessed = system_time_to_iso(metadata.accessed().ok());
    let created = system_time_to_iso(metadata.created().ok());

    conn.execute(
        "INSERT INTO nodes (snapshot_id, parent_id, name, type, size, mode, modified_at, accessed_at, created_at, blob_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![snapshot_id, parent_id, name, node_type, size, mode, modified, accessed, created, blob_hash],
    )?;
    Ok(conn.last_insert_rowid())
}

fn system_time_to_iso(time: Option<std::time::SystemTime>) -> Option<String> {
    time.map(|t| {
        let dt: DateTime<Utc> = t.into();
        dt.to_rfc3339()
    })
}

/// Build a GlobSet from exclude patterns.
///
/// Patterns use standard glob syntax: `*`, `?`, `**`, `{a,b}`.
/// If a pattern contains `/` or `**`, it is used as-is.
/// Otherwise it is auto-prefixed with `**/` so it matches at any depth
/// (e.g. `*.log` becomes `**/*.log`, `node_modules` becomes `**/node_modules`).
pub fn build_exclude_set(patterns: &[String]) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let effective = if pattern.contains('/') || pattern.contains("**") {
            pattern.clone()
        } else {
            format!("**/{pattern}")
        };
        let glob = Glob::new(&effective)
            .with_context(|| format!("invalid exclude pattern: {pattern}"))?;
        builder.add(glob);

        // Also match everything inside a matched directory
        let inside = format!("{effective}/**");
        if let Ok(g) = Glob::new(&inside) {
            builder.add(g);
        }
    }
    Ok(builder.build()?)
}

fn should_exclude(path: &Path, base: &Path, exclude_set: &GlobSet) -> bool {
    let rel = path.strip_prefix(base).unwrap_or(path);
    if rel.as_os_str().is_empty() {
        return false;
    }
    exclude_set.is_match(rel)
}

fn is_unchanged(conn: &Connection, prev_snapshot_id: i64, rel_path: &str, metadata: &fs::Metadata) -> Result<bool> {
    let name = Path::new(rel_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let result: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT size, modified_at FROM nodes WHERE snapshot_id = ?1 AND name = ?2 AND type = 'file' LIMIT 1",
            rusqlite::params![prev_snapshot_id, name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    if let Some((prev_size, prev_modified)) = result {
        let current_modified = system_time_to_iso(metadata.modified().ok());
        let current_size = metadata.len() as i64;
        Ok(prev_size == current_size && prev_modified == current_modified)
    } else {
        Ok(false)
    }
}

fn find_previous_hash(conn: &Connection, prev_snapshot_id: i64, rel_path: &str) -> Result<Option<String>> {
    let name = Path::new(rel_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let hash: Option<String> = conn
        .query_row(
            "SELECT blob_hash FROM nodes WHERE snapshot_id = ?1 AND name = ?2 AND type = 'file' LIMIT 1",
            rusqlite::params![prev_snapshot_id, name],
            |row| row.get(0),
        )
        .ok();

    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(patterns: &[&str], path: &str) -> bool {
        let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        let set = build_exclude_set(&pats).unwrap();
        let base = Path::new("/project");
        should_exclude(&base.join(path), base, &set)
    }

    #[test]
    fn test_extension_glob() {
        assert!(check(&["*.log"], "debug.log"));
        assert!(check(&["*.log"], "sub/dir/app.log"));
        assert!(!check(&["*.log"], "logfile.txt"));
    }

    #[test]
    fn test_directory_name() {
        assert!(check(&["node_modules"], "node_modules"));
        assert!(check(&["node_modules"], "a/node_modules"));
        assert!(check(&["node_modules"], "a/b/node_modules"));
        assert!(!check(&["node_modules"], "my_modules"));
    }

    #[test]
    fn test_double_star_pattern() {
        assert!(check(&["**/build/**"], "build/output.o"));
        assert!(check(&["**/build/**"], "src/build/output.o"));
        assert!(!check(&["**/build/**"], "rebuild/output.o"));
    }

    #[test]
    fn test_brace_expansion() {
        assert!(check(&["*.{tmp,bak}"], "file.tmp"));
        assert!(check(&["*.{tmp,bak}"], "file.bak"));
        assert!(!check(&["*.{tmp,bak}"], "file.log"));
    }

    #[test]
    fn test_path_with_slash() {
        // Pattern with / is used as-is (no auto-prefix)
        assert!(check(&["src/*.o"], "src/main.o"));
        assert!(!check(&["src/*.o"], "lib/main.o"));
    }

    #[test]
    fn test_root_path_not_excluded() {
        let pats: Vec<String> = vec!["*.log".to_string()];
        let set = build_exclude_set(&pats).unwrap();
        let base = Path::new("/project");
        assert!(!should_exclude(base, base, &set));
    }

    #[test]
    fn test_multiple_patterns() {
        assert!(check(&["*.log", "*.tmp"], "app.log"));
        assert!(check(&["*.log", "*.tmp"], "scratch.tmp"));
        assert!(!check(&["*.log", "*.tmp"], "readme.md"));
    }
}
