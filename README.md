# SQLite-FS

A cross-platform, portable file system built on SQLite for backup and archive. Stores files, directories, and metadata in a single `.sqlitefs` database file with built-in compression and deduplication.

## Features

- **Single-file archives** — everything lives in one portable `.sqlitefs` file
- **Compression** — Zstandard (default), zlib, or none with configurable levels
- **Content-addressable deduplication** — identical files stored only once via SHA-256 hashing
- **Incremental backups** — only back up changed files
- **Snapshots** — multiple point-in-time backups in one archive
- **Queryable** — search files by name, size, date, or run raw SQL
- **Integrity verification** — SHA-256 checksum validation for every blob
- **Cross-platform** — works on macOS, Linux, and Windows

## Installation

```bash
cargo install --path .
```

## Quick Start

```bash
# Back up a directory
sqlitefs backup ~/Documents --to docs.sqlitefs --label "april-backup"

# List snapshots
sqlitefs list docs.sqlitefs

# Browse files
sqlitefs ls docs.sqlitefs

# Restore everything
sqlitefs restore docs.sqlitefs --to ~/restored/

# Restore a single file
sqlitefs restore docs.sqlitefs --file "report.pdf" --to ~/restored/
```

## Commands

| Command | Description |
|---------|-------------|
| `backup` | Back up a directory into an archive |
| `restore` | Restore files from an archive |
| `list` | List all snapshots |
| `ls` | Browse files in a snapshot |
| `find` | Search for files by name or size |
| `history` | Show a file's history across snapshots |
| `info` | Show archive statistics |
| `verify` | Verify archive integrity |
| `prune` | Remove old snapshots |
| `compact` | Reclaim space after pruning |
| `query` | Run raw SQL against the archive |

## Compression

SQLite-FS supports three compression algorithms, selectable per backup:

```bash
# Zstandard (default, best balance of speed and ratio)
sqlitefs backup ./src --to archive.sqlitefs --compression zstd --level 3

# Zlib (broad compatibility)
sqlitefs backup ./src --to archive.sqlitefs --compression zlib --level 6

# No compression (fastest, for pre-compressed data)
sqlitefs backup ./src --to archive.sqlitefs --compression none
```

**Zstd levels 1–19** trade speed for compression ratio. Level 3 (default) is a good balance. Each blob records which algorithm was used, so archives can contain a mix.

## Incremental Backups

After the initial backup, use `--incremental` to skip unchanged files:

```bash
sqlitefs backup ./project --to project.sqlitefs --incremental
```

Files are compared by size and modification time against the previous snapshot.

## Exclusion Patterns

```bash
sqlitefs backup ./project --to project.sqlitefs \
  --exclude node_modules \
  --exclude "*.tmp" \
  --exclude .git
```

## Archive Management

```bash
# Show stats (file count, compression ratio, dedup savings)
sqlitefs info archive.sqlitefs

# Verify all blob checksums
sqlitefs verify archive.sqlitefs

# Keep only the last 5 snapshots
sqlitefs prune archive.sqlitefs --keep-last 5

# Reclaim disk space
sqlitefs compact archive.sqlitefs

# Run arbitrary SQL
sqlitefs query archive.sqlitefs "SELECT name, size FROM nodes WHERE size > 1000000"
```

## How It Works

SQLite-FS stores data in three core tables:

- **snapshots** — point-in-time backup metadata
- **nodes** — file/directory tree with permissions and timestamps
- **blobs** — compressed, content-addressable file data (deduplicated by SHA-256)

The database uses WAL mode for safe concurrent reads and ACID transactions for crash safety.

## License

MIT
