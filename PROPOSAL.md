# SQLite-FS: Cross-Platform SQLite-Based File System for Backup & Archive

## 1. Overview

**SQLite-FS** is a cross-platform, portable file system built on top of SQLite. It stores files, directories, and metadata inside a single `.sqlitefs` database file, providing a self-contained archive that can be created, read, and restored on any operating system—Windows, macOS, and Linux—without requiring special drivers or elevated privileges.

### Primary Goals

- **Backup**: Allow users to capture snapshots of their files into a single portable database.
- **Archive**: Provide long-term, self-describing storage that is resistant to file fragmentation and easy to transfer.
- **Cross-Platform**: Work identically on Windows, macOS, and Linux with zero platform-specific dependencies.

---

## 2. Problem Statement

Existing backup and archive solutions suffer from one or more of the following:

| Problem | Examples |
|---|---|
| Platform lock-in | NTFS-specific backups, macOS Time Machine bundles |
| Opaque binary formats | Proprietary `.bak` files that require specific software to restore |
| No built-in search/query | ZIP/TAR archives require full extraction to find a single file |
| No deduplication | Repeated backups store identical files multiple times |
| Corruption fragility | A single corrupt byte can render a ZIP archive unreadable |

SQLite-FS addresses all of these by leveraging SQLite—the most widely deployed database engine in the world—as its storage layer.

---

## 3. Architecture

```
┌─────────────────────────────────────────────────┐
│                   CLI / GUI                     │
│            (sqlitefs command-line tool)          │
├─────────────────────────────────────────────────┤
│               Core Library (API)                │
│   ┌───────────┬────────────┬──────────────────┐ │
│   │  Ingester  │  Restorer  │  Query Engine   │ │
│   └───────────┴────────────┴──────────────────┘ │
├─────────────────────────────────────────────────┤
│            Storage Engine (SQLite)              │
│   ┌───────────┬────────────┬──────────────────┐ │
│   │   nodes   │   blobs    │   snapshots      │ │
│   │  (files & │  (content  │  (backup sets)   │ │
│   │   dirs)   │   chunks)  │                  │ │
│   └───────────┴────────────┴──────────────────┘ │
├─────────────────────────────────────────────────┤
│          Single .sqlitefs database file         │
└─────────────────────────────────────────────────┘
```

### 3.1 Database Schema (Core Tables)

```sql
-- Snapshots represent a point-in-time backup
CREATE TABLE snapshots (
    id          INTEGER PRIMARY KEY,
    label       TEXT,                          -- e.g. "daily-2026-04-03"
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    source_path TEXT NOT NULL,                 -- original root path
    notes       TEXT
);

-- Nodes represent files and directories
CREATE TABLE nodes (
    id          INTEGER PRIMARY KEY,
    snapshot_id INTEGER NOT NULL REFERENCES snapshots(id),
    parent_id   INTEGER REFERENCES nodes(id),  -- NULL for root
    name        TEXT NOT NULL,                  -- file or directory name
    type        TEXT NOT NULL CHECK (type IN ('file', 'directory', 'symlink')),
    size        INTEGER DEFAULT 0,             -- uncompressed size in bytes
    mode        INTEGER,                       -- POSIX permissions (portable)
    owner       TEXT,
    grp         TEXT,
    created_at  TEXT,
    modified_at TEXT,
    accessed_at TEXT,
    blob_hash   TEXT,                          -- SHA-256 hash, references blobs
    extra       TEXT                           -- JSON for platform-specific metadata
);

-- Content-addressable blob storage with deduplication
CREATE TABLE blobs (
    hash        TEXT PRIMARY KEY,              -- SHA-256 of uncompressed content
    data        BLOB NOT NULL,                 -- compressed content (zstd)
    size_raw    INTEGER NOT NULL,              -- original size
    size_stored INTEGER NOT NULL,              -- compressed size
    compression TEXT NOT NULL DEFAULT 'zstd'   -- compression algorithm used
);

-- Full-text search on file paths and metadata
CREATE VIRTUAL TABLE nodes_fts USING fts5(name, full_path, extra);

-- Indexes
CREATE INDEX idx_nodes_snapshot ON nodes(snapshot_id);
CREATE INDEX idx_nodes_parent ON nodes(parent_id);
CREATE INDEX idx_nodes_hash ON nodes(blob_hash);
CREATE INDEX idx_nodes_name ON nodes(name);
```

### 3.2 Key Design Decisions

| Decision | Rationale |
|---|---|
| **Content-addressable blobs** | Files are stored by SHA-256 hash. If 10 snapshots contain the same file, only one copy is stored. |
| **Zstandard compression** | High compression ratio with fast decompression. Falls back to `zlib` for broad compatibility. |
| **Chunked large files** | Files over 64 MB are split into chunks to stay within SQLite's practical blob limits. |
| **Single-file database** | The entire archive is one file—easy to copy, move, email, or store on any medium. |
| **WAL mode** | Write-Ahead Logging allows concurrent reads during backup operations. |

---

## 4. Core Features

### 4.1 Backup (Ingest)

```bash
# Full backup of a directory
sqlitefs backup ./my-project --to archive.sqlitefs --label "v1.0-release"

# Incremental backup (only changed files since last snapshot)
sqlitefs backup ./my-project --to archive.sqlitefs --incremental

# Backup with exclusion patterns
sqlitefs backup ./my-project --to archive.sqlitefs --exclude "node_modules" --exclude "*.tmp"
```

- Walks the source directory tree recursively.
- Computes SHA-256 for each file; skips blob insertion if hash already exists (deduplication).
- Incremental mode compares file modification times and sizes against the latest snapshot.

### 4.2 Restore

```bash
# Restore latest snapshot
sqlitefs restore archive.sqlitefs --to ./restored/

# Restore a specific snapshot
sqlitefs restore archive.sqlitefs --snapshot 3 --to ./restored/

# Restore a single file
sqlitefs restore archive.sqlitefs --file "src/main.py" --to ./restored/

# Dry-run (show what would be restored)
sqlitefs restore archive.sqlitefs --to ./restored/ --dry-run
```

### 4.3 Query & Browse

```bash
# List all snapshots
sqlitefs list archive.sqlitefs

# Browse files in a snapshot
sqlitefs ls archive.sqlitefs --snapshot 2 --path "/src"

# Search for files by name
sqlitefs find archive.sqlitefs --name "*.py"

# Search file metadata
sqlitefs find archive.sqlitefs --larger-than 10MB --modified-after "2026-01-01"

# Show file history across snapshots
sqlitefs history archive.sqlitefs --file "src/main.py"

# Direct SQL access for power users
sqlitefs query archive.sqlitefs "SELECT name, size FROM nodes WHERE size > 1000000"
```

### 4.4 Archive Management

```bash
# Show archive statistics
sqlitefs info archive.sqlitefs

# Verify archive integrity (checksum validation)
sqlitefs verify archive.sqlitefs

# Prune old snapshots (keep last N)
sqlitefs prune archive.sqlitefs --keep-last 5

# Compact database (reclaim space after pruning)
sqlitefs compact archive.sqlitefs

# Export a snapshot to a standard ZIP/TAR
sqlitefs export archive.sqlitefs --snapshot 2 --format zip --to backup.zip
```

---

## 5. Cross-Platform Strategy

| Aspect | Approach |
|---|---|
| **Path separators** | Stored as POSIX-style (`/`) internally; converted to native format on restore. |
| **File permissions** | Stored as POSIX mode integers. On Windows, mapped to read-only / read-write attributes. |
| **Timestamps** | Stored as ISO 8601 UTC strings for portability. |
| **Symlinks** | Stored with target path. On Windows, restored as junction points or shortcuts where possible. |
| **Extended attributes** | Stored in the `extra` JSON column (xattrs on Linux/macOS, alternate data streams info on Windows). |
| **Character encoding** | All text stored as UTF-8. File names normalized to NFC form. |

---

## 6. Technical Specifications

| Specification | Value |
|---|---|
| Language | Rust (core library) with Python and Node.js bindings |
| SQLite version | 3.45+ (bundled, not system-dependent) |
| Max archive size | Up to **281 TB** (SQLite's theoretical limit) |
| Max file size | Unlimited (chunked at 64 MB boundaries) |
| Compression | Zstandard (level 3 default, configurable 1–19) |
| Hashing | SHA-256 |
| Concurrency | Safe concurrent reads; exclusive write lock per archive |
| Platforms | Windows (x86_64, ARM64), macOS (x86_64, ARM64), Linux (x86_64, ARM64, musl) |

---

## 7. Security

- **Encryption at rest** (optional): AES-256-GCM encryption of blob data using a user-provided passphrase (key derived via Argon2id).
- **Integrity verification**: SHA-256 checksums for every stored blob, verified on restore.
- **SQLite integrity checks**: Built-in `PRAGMA integrity_check` exposed via `sqlitefs verify`.
- **No remote execution**: The tool operates purely on local files—no network access, no telemetry.

---

## 8. Performance Targets

| Operation | Target |
|---|---|
| Full backup (10 GB, 50k files) | < 3 minutes |
| Incremental backup (10 GB, 100 changed files) | < 10 seconds |
| Single file restore | < 1 second |
| Full restore (10 GB) | < 2 minutes |
| File search (50k files) | < 500 ms |
| Archive verification (10 GB) | < 1 minute |

---

## 9. Future Roadmap

1. **FUSE / WinFSP mount** — Mount `.sqlitefs` archives as virtual read-only file systems.
2. **Cloud sync** — Push/pull archives to S3, GCS, or Azure Blob Storage.
3. **Scheduled backups** — Built-in cron-like scheduler with retention policies.
4. **GUI application** — Electron or Tauri-based visual browser for archives.
5. **Diff viewer** — Compare two snapshots and show added/removed/modified files.
6. **Encryption key management** — Multi-key support, key rotation.

---

## 10. Why SQLite?

> *"SQLite is the most used database engine in the world."*
> — [sqlite.org](https://sqlite.org)

- **Battle-tested**: Billions of deployments across every platform.
- **Zero configuration**: No server, no setup, no administration.
- **Single-file format**: The archive is just one file—copy it anywhere.
- **ACID transactions**: Backup operations are atomic; a crash mid-backup won't corrupt the archive.
- **Queryable**: Unlike ZIP/TAR, users can search, filter, and analyze their archives with SQL.
- **Long-term format**: SQLite's file format is [guaranteed stable until 2050](https://www.sqlite.org/lts.html).

---

## 11. Getting Started (Planned)

```bash
# Install
cargo install sqlitefs

# Or via package managers
brew install sqlitefs          # macOS
winget install sqlitefs        # Windows
apt install sqlitefs           # Debian/Ubuntu

# Create your first backup
sqlitefs backup ~/Documents --to documents.sqlitefs --label "initial"

# Check what's inside
sqlitefs info documents.sqlitefs

# Restore later, anywhere
sqlitefs restore documents.sqlitefs --to ~/restored-docs/
```

---

## 12. License

Proposed: **MIT License** — permissive, compatible with SQLite's public domain status.

---

*SQLite-FS — Your files, one database, every platform.*
