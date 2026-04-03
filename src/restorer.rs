use anyhow::{Result, Context};
use rusqlite::Connection;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::blob;

pub struct RestoreOptions {
    pub snapshot_id: Option<i64>,
    pub file_path: Option<String>,
    pub dry_run: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            snapshot_id: None,
            file_path: None,
            dry_run: false,
        }
    }
}

pub struct RestoreStats {
    pub files: u64,
    pub directories: u64,
    pub bytes_restored: u64,
}

/// Restore a snapshot to the target directory.
pub fn restore(conn: &Connection, target: &str, opts: &RestoreOptions) -> Result<RestoreStats> {
    let target_path = Path::new(target);

    // Determine which snapshot to restore
    let snapshot_id = match opts.snapshot_id {
        Some(id) => id,
        None => conn
            .query_row("SELECT id FROM snapshots ORDER BY id DESC LIMIT 1", [], |row| row.get(0))
            .with_context(|| "no snapshots found in archive")?,
    };

    // Verify snapshot exists
    let _label: Option<String> = conn.query_row(
        "SELECT label FROM snapshots WHERE id = ?1",
        [snapshot_id],
        |row| row.get(0),
    ).with_context(|| format!("snapshot {} not found", snapshot_id))?;

    let mut stats = RestoreStats {
        files: 0,
        directories: 0,
        bytes_restored: 0,
    };

    if opts.file_path.is_some() {
        restore_single_file(conn, target_path, snapshot_id, opts, &mut stats)?;
    } else {
        restore_full(conn, target_path, snapshot_id, opts, &mut stats)?;
    }

    Ok(stats)
}

fn restore_full(
    conn: &Connection,
    target: &Path,
    snapshot_id: i64,
    opts: &RestoreOptions,
    stats: &mut RestoreStats,
) -> Result<()> {
    // First, restore directories (ordered by id to get parents first)
    let mut dir_stmt = conn.prepare(
        "SELECT id, parent_id, name, mode FROM nodes WHERE snapshot_id = ?1 AND type = 'directory' ORDER BY id"
    )?;

    let mut node_to_path: std::collections::HashMap<i64, std::path::PathBuf> = std::collections::HashMap::new();

    let dirs: Vec<(i64, Option<i64>, String, Option<i64>)> = dir_stmt
        .query_map([snapshot_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (id, parent_id, name, mode) in &dirs {
        // The root "." directory maps to the target directory itself
        let dir_path = if name == "." && parent_id.is_none() {
            target.to_path_buf()
        } else if let Some(pid) = parent_id {
            node_to_path.get(pid).map(|p| p.join(name)).unwrap_or_else(|| target.join(name))
        } else {
            target.join(name)
        };

        if !opts.dry_run {
            fs::create_dir_all(&dir_path)
                .with_context(|| format!("failed to create directory: {}", dir_path.display()))?;
            if let Some(m) = mode {
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(&dir_path, fs::Permissions::from_mode(*m as u32));
                }
            }
        }

        stats.directories += 1;
        node_to_path.insert(*id, dir_path);
    }

    // Then restore files
    let mut file_stmt = conn.prepare(
        "SELECT id, parent_id, name, size, mode, blob_hash FROM nodes WHERE snapshot_id = ?1 AND type = 'file' ORDER BY id"
    )?;

    let files: Vec<(i64, Option<i64>, String, i64, Option<i64>, Option<String>)> = file_stmt
        .query_map([snapshot_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (_id, parent_id, name, size, mode, blob_hash) in &files {
        let file_path = if let Some(pid) = parent_id {
            node_to_path.get(pid).map(|p| p.join(name)).unwrap_or_else(|| target.join(name))
        } else {
            target.join(name)
        };

        if opts.dry_run {
            println!("  {} ({} bytes)", file_path.display(), size);
        } else {
            // Ensure parent directory exists
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }

            if let Some(hash) = blob_hash {
                let data = blob::retrieve(conn, hash)
                    .with_context(|| format!("failed to retrieve blob for: {}", file_path.display()))?;
                fs::write(&file_path, &data)?;
                stats.bytes_restored += data.len() as u64;
            } else {
                // Empty file
                fs::write(&file_path, b"")?;
            }

            if let Some(m) = mode {
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(&file_path, fs::Permissions::from_mode(*m as u32));
                }
            }
        }

        stats.files += 1;
    }

    Ok(())
}

fn restore_single_file(
    conn: &Connection,
    target: &Path,
    snapshot_id: i64,
    opts: &RestoreOptions,
    stats: &mut RestoreStats,
) -> Result<()> {
    let file_name = opts.file_path.as_ref().unwrap();
    let search_name = Path::new(file_name)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.clone());

    let (blob_hash, size): (Option<String>, i64) = conn
        .query_row(
            "SELECT blob_hash, size FROM nodes WHERE snapshot_id = ?1 AND name = ?2 AND type = 'file' LIMIT 1",
            rusqlite::params![snapshot_id, search_name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .with_context(|| format!("file '{}' not found in snapshot {}", file_name, snapshot_id))?;

    let output_path = target.join(&search_name);

    if opts.dry_run {
        println!("  {} ({} bytes)", output_path.display(), size);
    } else {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }

        if let Some(hash) = &blob_hash {
            let data = blob::retrieve(conn, hash)?;
            fs::write(&output_path, &data)?;
            stats.bytes_restored += data.len() as u64;
        } else {
            fs::write(&output_path, b"")?;
        }
    }

    stats.files += 1;
    Ok(())
}
