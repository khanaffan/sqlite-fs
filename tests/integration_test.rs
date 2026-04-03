use std::fs;
use tempfile::TempDir;

use sqlitefs::{compression, db, ingester, query, restorer};

fn create_test_files(dir: &std::path::Path) {
    fs::create_dir_all(dir.join("subdir")).unwrap();
    fs::write(dir.join("hello.txt"), "Hello, World!").unwrap();
    fs::write(dir.join("data.bin"), vec![0u8; 1024]).unwrap();
    fs::write(dir.join("subdir/nested.txt"), "Nested file content").unwrap();
    fs::write(
        dir.join("subdir/large.txt"),
        "Large file content. ".repeat(5000),
    )
    .unwrap();
}

fn backup_with_defaults(
    conn: &rusqlite::Connection,
    source: &std::path::Path,
    label: Option<&str>,
) -> ingester::BackupStats {
    let opts = ingester::BackupOptions {
        label: label.map(|s| s.to_string()),
        ..Default::default()
    };
    ingester::backup(conn, source.to_str().unwrap(), &opts).unwrap()
}

#[test]
fn test_backup_and_restore_roundtrip() {
    let source_dir = TempDir::new().unwrap();
    let restore_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("test.sqlitefs");

    create_test_files(source_dir.path());

    // Backup
    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions {
        label: Some("test-backup".to_string()),
        compression: compression::Config::default(),
        ..Default::default()
    };
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert!(stats.files >= 3);
    assert!(stats.directories >= 1);
    assert!(stats.bytes_raw > 0);
    assert!(stats.bytes_stored > 0);
    // Compression should reduce stored size for the large file
    assert!(stats.bytes_stored < stats.bytes_raw);

    // Restore
    let restore_opts = restorer::RestoreOptions::default();
    let restore_stats =
        restorer::restore(&conn, restore_dir.path().to_str().unwrap(), &restore_opts).unwrap();

    assert!(restore_stats.files >= 3);

    // Verify file contents match
    let original = fs::read_to_string(source_dir.path().join("hello.txt")).unwrap();
    // Find the restored hello.txt (it may be in a subdirectory)
    let restored_hello = find_file(restore_dir.path(), "hello.txt");
    assert!(restored_hello.is_some(), "hello.txt not found in restore");
    let restored = fs::read_to_string(restored_hello.unwrap()).unwrap();
    assert_eq!(original, restored);
}

#[test]
fn test_compression_algorithms() {
    for algo in &["zstd", "zlib", "none"] {
        let source_dir = TempDir::new().unwrap();
        let db_dir = TempDir::new().unwrap();
        let db_path = db_dir.path().join(format!("test_{}.sqlitefs", algo));

        fs::write(
            source_dir.path().join("test.txt"),
            "Compression test data. ".repeat(1000),
        )
        .unwrap();

        let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
        let comp = sqlitefs::cli::parse_compression(algo, 3).unwrap();
        let opts = ingester::BackupOptions {
            label: Some(format!("{}-test", algo)),
            compression: comp,
            ..Default::default()
        };

        let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();
        assert!(stats.files >= 1, "algo={}: no files backed up", algo);

        // Verify restore works
        let restore_dir = TempDir::new().unwrap();
        restorer::restore(
            &conn,
            restore_dir.path().to_str().unwrap(),
            &restorer::RestoreOptions::default(),
        )
        .unwrap();

        let restored = find_file(restore_dir.path(), "test.txt");
        assert!(restored.is_some(), "algo={}: test.txt not found", algo);
        let content = fs::read_to_string(restored.unwrap()).unwrap();
        assert_eq!(content, "Compression test data. ".repeat(1000));
    }
}

#[test]
fn test_deduplication() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("dedup.sqlitefs");

    // Create two identical files
    let content = "Identical content for dedup test";
    fs::write(source_dir.path().join("file1.txt"), content).unwrap();
    fs::write(source_dir.path().join("file2.txt"), content).unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions::default();
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats.files, 2);
    assert_eq!(stats.blobs_deduped, 1); // One blob was deduped

    // Only 1 unique blob should exist
    let blob_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(blob_count, 1);
}

#[test]
fn test_snapshots_and_query() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("query.sqlitefs");

    fs::write(source_dir.path().join("readme.md"), "# Hello").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();

    // Create two snapshots
    let opts1 = ingester::BackupOptions {
        label: Some("v1".to_string()),
        ..Default::default()
    };
    ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts1).unwrap();

    fs::write(source_dir.path().join("readme.md"), "# Updated").unwrap();
    let opts2 = ingester::BackupOptions {
        label: Some("v2".to_string()),
        ..Default::default()
    };
    ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts2).unwrap();

    let snapshots = query::list_snapshots(&conn).unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].label.as_deref(), Some("v1"));
    assert_eq!(snapshots[1].label.as_deref(), Some("v2"));

    // Find files
    let found = query::find_files(&conn, "*.md").unwrap();
    assert!(found.len() >= 2); // readme.md in both snapshots

    // File history
    let history = query::file_history(&conn, "readme.md").unwrap();
    assert!(history.len() >= 2);
}

#[test]
fn test_verify_integrity() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("verify.sqlitefs");

    fs::write(source_dir.path().join("test.txt"), "Verify me").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions::default();
    ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    let (ok, failed) = query::verify(&conn).unwrap();
    assert!(ok >= 1);
    assert_eq!(failed, 0);
}

#[test]
fn test_prune() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("prune.sqlitefs");

    fs::write(source_dir.path().join("test.txt"), "Prune test").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();

    // Create 3 snapshots
    for i in 1..=3 {
        let opts = ingester::BackupOptions {
            label: Some(format!("snap-{}", i)),
            ..Default::default()
        };
        ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();
    }

    assert_eq!(query::list_snapshots(&conn).unwrap().len(), 3);

    // Keep last 1
    let deleted = query::prune(&conn, 1).unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(query::list_snapshots(&conn).unwrap().len(), 1);
}

fn find_file(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in walkdir::WalkDir::new(dir) {
        if let Ok(e) = entry {
            if e.file_name().to_string_lossy() == name {
                return Some(e.path().to_path_buf());
            }
        }
    }
    None
}

fn list_all_files(dir: &std::path::Path) -> Vec<String> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            e.path()
                .strip_prefix(dir)
                .unwrap_or(e.path())
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

// ─── Exclude / Ignore Tests ───

#[test]
fn test_exclude_by_extension() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_ext.sqlitefs");

    fs::write(source_dir.path().join("keep.txt"), "keep").unwrap();
    fs::write(source_dir.path().join("remove.log"), "log data").unwrap();
    fs::write(source_dir.path().join("also_remove.log"), "more logs").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions {
        exclude: vec!["*.log".to_string()],
        ..Default::default()
    };
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats.files, 1, "only keep.txt should be backed up");

    let restore_dir = TempDir::new().unwrap();
    restorer::restore(
        &conn,
        restore_dir.path().to_str().unwrap(),
        &restorer::RestoreOptions::default(),
    )
    .unwrap();

    let files = list_all_files(restore_dir.path());
    assert!(files.iter().any(|f| f.contains("keep.txt")));
    assert!(!files.iter().any(|f| f.contains(".log")));
}

#[test]
fn test_exclude_directory_name() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_dir.sqlitefs");

    fs::create_dir_all(source_dir.path().join("node_modules/pkg")).unwrap();
    fs::write(source_dir.path().join("index.js"), "main").unwrap();
    fs::write(
        source_dir.path().join("node_modules/pkg/lib.js"),
        "dep",
    )
    .unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions {
        exclude: vec!["node_modules".to_string()],
        ..Default::default()
    };
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats.files, 1, "only index.js should be backed up");
}

#[test]
fn test_exclude_nested_glob() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_nested.sqlitefs");

    fs::create_dir_all(source_dir.path().join("src/build")).unwrap();
    fs::create_dir_all(source_dir.path().join("docs")).unwrap();
    fs::write(source_dir.path().join("src/main.rs"), "fn main()").unwrap();
    fs::write(source_dir.path().join("src/build/output.o"), "binary").unwrap();
    fs::write(source_dir.path().join("docs/readme.md"), "docs").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions {
        exclude: vec!["**/build/**".to_string()],
        ..Default::default()
    };
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats.files, 2, "main.rs and readme.md should be backed up");
}

#[test]
fn test_exclude_multiple_patterns() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_multi.sqlitefs");

    fs::write(source_dir.path().join("app.rs"), "code").unwrap();
    fs::write(source_dir.path().join("debug.log"), "log").unwrap();
    fs::write(source_dir.path().join("temp.tmp"), "tmp").unwrap();
    fs::write(source_dir.path().join("backup.bak"), "bak").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions {
        exclude: vec![
            "*.log".to_string(),
            "*.tmp".to_string(),
            "*.bak".to_string(),
        ],
        ..Default::default()
    };
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats.files, 1, "only app.rs should be backed up");
}

#[test]
fn test_exclude_brace_expansion() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_brace.sqlitefs");

    fs::write(source_dir.path().join("main.rs"), "code").unwrap();
    fs::write(source_dir.path().join("old.tmp"), "temp").unwrap();
    fs::write(source_dir.path().join("old.bak"), "backup").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions {
        exclude: vec!["*.{tmp,bak}".to_string()],
        ..Default::default()
    };
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats.files, 1, "only main.rs should be backed up");
}

#[test]
fn test_exclude_path_scoped_pattern() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_scoped.sqlitefs");

    fs::create_dir_all(source_dir.path().join("src")).unwrap();
    fs::create_dir_all(source_dir.path().join("lib")).unwrap();
    fs::write(source_dir.path().join("src/main.o"), "obj").unwrap();
    fs::write(source_dir.path().join("lib/util.o"), "obj").unwrap();
    fs::write(source_dir.path().join("src/main.rs"), "code").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    // Only exclude .o files under src/, not lib/
    let opts = ingester::BackupOptions {
        exclude: vec!["src/*.o".to_string()],
        ..Default::default()
    };
    let stats = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats.files, 2, "main.rs and lib/util.o should be backed up");
}

#[test]
fn test_exclude_no_patterns_includes_all() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_none.sqlitefs");

    fs::write(source_dir.path().join("a.txt"), "a").unwrap();
    fs::write(source_dir.path().join("b.log"), "b").unwrap();
    fs::write(source_dir.path().join("c.tmp"), "c").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let stats = backup_with_defaults(&conn, source_dir.path(), None);

    assert_eq!(stats.files, 3, "all files should be backed up with no excludes");
}

#[test]
fn test_exclude_roundtrip_restore() {
    let source_dir = TempDir::new().unwrap();
    let restore_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("exclude_rt.sqlitefs");

    fs::create_dir_all(source_dir.path().join("logs")).unwrap();
    fs::write(source_dir.path().join("app.txt"), "important data").unwrap();
    fs::write(source_dir.path().join("logs/err.log"), "error log").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();
    let opts = ingester::BackupOptions {
        exclude: vec!["*.log".to_string()],
        ..Default::default()
    };
    ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    restorer::restore(
        &conn,
        restore_dir.path().to_str().unwrap(),
        &restorer::RestoreOptions::default(),
    )
    .unwrap();

    let restored = find_file(restore_dir.path(), "app.txt");
    assert!(restored.is_some(), "app.txt should be restored");
    let content = fs::read_to_string(restored.unwrap()).unwrap();
    assert_eq!(content, "important data");

    assert!(
        find_file(restore_dir.path(), "err.log").is_none(),
        "excluded err.log should not be restored"
    );
}

// ─── Incremental Backup Tests ───

#[test]
fn test_incremental_backup() {
    let source_dir = TempDir::new().unwrap();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("incremental.sqlitefs");

    fs::write(source_dir.path().join("stable.txt"), "unchanged").unwrap();
    fs::write(source_dir.path().join("changing.txt"), "v1").unwrap();

    let conn = db::open_or_create(db_path.to_str().unwrap()).unwrap();

    // First full backup
    let stats1 = backup_with_defaults(&conn, source_dir.path(), Some("full"));
    assert_eq!(stats1.files, 2);

    // Modify one file, then incremental
    fs::write(source_dir.path().join("changing.txt"), "v2").unwrap();
    let opts = ingester::BackupOptions {
        label: Some("incremental".to_string()),
        incremental: true,
        ..Default::default()
    };
    let stats2 = ingester::backup(&conn, source_dir.path().to_str().unwrap(), &opts).unwrap();

    assert_eq!(stats2.files, 2);
    // stable.txt should be deduped (reused from previous snapshot)
    assert!(stats2.blobs_deduped >= 1, "unchanged file should be deduped");
}
