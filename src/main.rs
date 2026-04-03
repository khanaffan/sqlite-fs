use anyhow::Result;
use clap::Parser;

use sqlitefs::cli::{self, Cli, Command};
use sqlitefs::{db, ingester, query, restorer};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Backup {
            source,
            archive,
            label,
            notes,
            compression,
            level,
            exclude,
            incremental,
        } => {
            let comp_config = cli::parse_compression(&compression, level)?;
            let conn = db::open_or_create(&archive)?;
            let opts = ingester::BackupOptions {
                label,
                notes,
                compression: comp_config,
                exclude,
                incremental,
            };
            let stats = ingester::backup(&conn, &source, &opts)?;
            println!("\nSnapshot #{} created:", stats.snapshot_id);
            println!("  Files:       {}", stats.files);
            println!("  Directories: {}", stats.directories);
            println!("  Raw size:    {}", format_size(stats.bytes_raw));
            println!("  Stored size: {}", format_size(stats.bytes_stored));
            println!("  Deduplicated: {} blobs", stats.blobs_deduped);
            if stats.bytes_raw > 0 {
                let ratio = stats.bytes_stored as f64 / stats.bytes_raw as f64;
                println!("  Compression: {:.1}%", (1.0 - ratio) * 100.0);
            }
        }

        Command::Restore {
            archive,
            target,
            snapshot,
            file,
            dry_run,
        } => {
            let conn = db::open_or_create(&archive)?;
            let opts = restorer::RestoreOptions {
                snapshot_id: snapshot,
                file_path: file,
                dry_run,
            };
            if dry_run {
                println!("Dry run — files that would be restored:");
            }
            let stats = restorer::restore(&conn, &target, &opts)?;
            if !dry_run {
                println!("Restored:");
                println!("  Files:       {}", stats.files);
                println!("  Directories: {}", stats.directories);
                println!("  Total size:  {}", format_size(stats.bytes_restored));
            }
        }

        Command::List { archive } => {
            let conn = db::open_or_create(&archive)?;
            let snapshots = query::list_snapshots(&conn)?;
            if snapshots.is_empty() {
                println!("No snapshots found.");
            } else {
                println!("{:<4} {:<20} {:<24} {:<8} {:<10} {}", "ID", "Label", "Created", "Files", "Size", "Source");
                println!("{}", "-".repeat(90));
                for s in &snapshots {
                    println!(
                        "{:<4} {:<20} {:<24} {:<8} {:<10} {}",
                        s.id,
                        s.label.as_deref().unwrap_or("-"),
                        s.created_at,
                        s.file_count,
                        format_size(s.total_size as u64),
                        s.source_path,
                    );
                }
            }
        }

        Command::Ls { archive, snapshot } => {
            let conn = db::open_or_create(&archive)?;
            let sid = snapshot.unwrap_or_else(|| {
                conn.query_row("SELECT id FROM snapshots ORDER BY id DESC LIMIT 1", [], |row| row.get(0))
                    .unwrap_or(1)
            });
            let files = query::list_files(&conn, sid, None)?;
            for f in &files {
                let type_indicator = match f.node_type.as_str() {
                    "directory" => "d",
                    "symlink" => "l",
                    _ => "-",
                };
                println!("{} {:>10} {}", type_indicator, format_size(f.size as u64), f.name);
            }
        }

        Command::Find { archive, name, larger_than } => {
            let conn = db::open_or_create(&archive)?;
            if let Some(pattern) = name {
                let results = query::find_files(&conn, &pattern)?;
                for (sid, name, size) in &results {
                    println!("  [snapshot {}] {} ({})", sid, name, format_size(*size as u64));
                }
                println!("{} file(s) found", results.len());
            } else if let Some(size_str) = larger_than {
                let min_size = cli::parse_size(&size_str)?;
                let sid: i64 = conn.query_row("SELECT id FROM snapshots ORDER BY id DESC LIMIT 1", [], |row| row.get(0))?;
                let results = query::find_larger_than(&conn, sid, min_size)?;
                for f in &results {
                    println!("  {} ({})", f.name, format_size(f.size as u64));
                }
                println!("{} file(s) found", results.len());
            }
        }

        Command::History { archive, file } => {
            let conn = db::open_or_create(&archive)?;
            let history = query::file_history(&conn, &file)?;
            if history.is_empty() {
                println!("No history found for '{}'", file);
            } else {
                println!("{:<8} {:<24} {:<10} {}", "Snap", "Date", "Size", "Hash");
                println!("{}", "-".repeat(70));
                for (sid, date, size, hash) in &history {
                    println!(
                        "{:<8} {:<24} {:<10} {}",
                        sid, date, format_size(*size as u64),
                        hash.as_deref().unwrap_or("-"),
                    );
                }
            }
        }

        Command::Info { archive } => {
            let conn = db::open_or_create(&archive)?;
            let info = query::archive_info(&conn, &archive)?;
            println!("Archive: {}", archive);
            println!("  Snapshots:     {}", info.snapshot_count);
            println!("  Total files:   {}", info.total_files);
            println!("  Total dirs:    {}", info.total_dirs);
            println!("  Unique blobs:  {}", info.total_blobs);
            println!("  Raw size:      {}", format_size(info.total_raw_bytes as u64));
            println!("  Stored size:   {}", format_size(info.total_stored_bytes as u64));
            println!("  DB file size:  {}", format_size(info.db_size as u64));
            if info.total_raw_bytes > 0 {
                let ratio = info.total_stored_bytes as f64 / info.total_raw_bytes as f64;
                println!("  Compression:   {:.1}%", (1.0 - ratio) * 100.0);
            }
        }

        Command::Verify { archive } => {
            let conn = db::open_or_create(&archive)?;
            println!("Verifying archive integrity...");
            let (ok, failed) = query::verify(&conn)?;
            println!("  OK:     {}", ok);
            println!("  Failed: {}", failed);
            if failed > 0 {
                std::process::exit(1);
            } else {
                println!("All blobs verified successfully.");
            }
        }

        Command::Prune { archive, keep_last } => {
            let conn = db::open_or_create(&archive)?;
            let deleted = query::prune(&conn, keep_last)?;
            println!("Pruned {} snapshot(s), kept last {}", deleted, keep_last);
        }

        Command::Compact { archive } => {
            let conn = db::open_or_create(&archive)?;
            println!("Compacting database...");
            query::compact(&conn)?;
            println!("Done.");
        }

        Command::Query { archive, sql } => {
            let conn = db::open_or_create(&archive)?;
            let results = query::raw_query(&conn, &sql)?;
            for (i, row) in results.iter().enumerate() {
                if i == 0 {
                    println!("{}", row.join("\t"));
                    println!("{}", "-".repeat(row.iter().map(|c| c.len() + 1).sum::<usize>()));
                } else {
                    println!("{}", row.join("\t"));
                }
            }
        }
    }

    Ok(())
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

