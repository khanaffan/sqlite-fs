use clap::{Parser, Subcommand};

use crate::compression;

#[derive(Parser)]
#[command(name = "sqlitefs", about = "SQLite-based file system for backup & archive", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Backup a directory into an archive
    Backup {
        /// Source directory to back up
        source: String,
        /// Path to the archive file
        #[arg(long = "to")]
        archive: String,
        /// Label for the snapshot
        #[arg(long)]
        label: Option<String>,
        /// Notes for the snapshot
        #[arg(long)]
        notes: Option<String>,
        /// Compression algorithm (zstd, zlib, none)
        #[arg(long, default_value = "zstd")]
        compression: String,
        /// Compression level
        #[arg(long, default_value_t = 3)]
        level: i32,
        /// Exclude patterns
        #[arg(long)]
        exclude: Vec<String>,
        /// Incremental backup (only changed files)
        #[arg(long)]
        incremental: bool,
    },

    /// Restore from an archive
    Restore {
        /// Path to the archive file
        archive: String,
        /// Target directory for restoration
        #[arg(long = "to")]
        target: String,
        /// Specific snapshot ID to restore
        #[arg(long)]
        snapshot: Option<i64>,
        /// Restore a single file
        #[arg(long)]
        file: Option<String>,
        /// Show what would be restored without writing
        #[arg(long)]
        dry_run: bool,
    },

    /// List all snapshots in an archive
    List {
        /// Path to the archive file
        archive: String,
    },

    /// Browse files in a snapshot
    Ls {
        /// Path to the archive file
        archive: String,
        /// Snapshot ID
        #[arg(long)]
        snapshot: Option<i64>,
    },

    /// Search for files by name pattern
    Find {
        /// Path to the archive file
        archive: String,
        /// File name pattern (supports * and ?)
        #[arg(long)]
        name: Option<String>,
        /// Find files larger than this size (e.g. 10MB)
        #[arg(long)]
        larger_than: Option<String>,
    },

    /// Show file history across snapshots
    History {
        /// Path to the archive file
        archive: String,
        /// File path to trace
        #[arg(long)]
        file: String,
    },

    /// Show archive statistics
    Info {
        /// Path to the archive file
        archive: String,
    },

    /// Verify archive integrity
    Verify {
        /// Path to the archive file
        archive: String,
    },

    /// Prune old snapshots
    Prune {
        /// Path to the archive file
        archive: String,
        /// Number of recent snapshots to keep
        #[arg(long)]
        keep_last: usize,
    },

    /// Compact the database (reclaim space)
    Compact {
        /// Path to the archive file
        archive: String,
    },

    /// Execute a raw SQL query
    Query {
        /// Path to the archive file
        archive: String,
        /// SQL query to execute
        sql: String,
    },
}

pub fn parse() -> Cli {
    Cli::parse()
}

pub fn parse_compression(algorithm: &str, level: i32) -> anyhow::Result<compression::Config> {
    let algo = compression::Algorithm::from_str(algorithm)?;
    Ok(compression::Config {
        algorithm: algo,
        level,
    })
}

pub fn parse_size(s: &str) -> anyhow::Result<i64> {
    let s = s.trim().to_uppercase();
    if let Some(num) = s.strip_suffix("GB") {
        Ok(num.trim().parse::<i64>()? * 1024 * 1024 * 1024)
    } else if let Some(num) = s.strip_suffix("MB") {
        Ok(num.trim().parse::<i64>()? * 1024 * 1024)
    } else if let Some(num) = s.strip_suffix("KB") {
        Ok(num.trim().parse::<i64>()? * 1024)
    } else if let Some(num) = s.strip_suffix('B') {
        Ok(num.trim().parse::<i64>()?)
    } else {
        Ok(s.parse::<i64>()?)
    }
}
