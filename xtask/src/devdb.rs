//! Local SQLite database housekeeping.
//!
//! Halation uses embedded SQLite (`data/halation.db` in local dev) and creates
//! and migrates the database automatically on boot, so no Docker container is
//! required. This command ensures `data/` exists and, with `--clean`, resets
//! the local dev database and removes any leftover temp test files.

use std::path::Path;

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let clean = args.iter().any(|a| a == "--clean" || a == "-Clean");
    let unknown: Vec<&String> = args
        .iter()
        .filter(|a| *a != "--clean" && *a != "-Clean")
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown option(s): {unknown:?}. Usage: cargo xtask dev-db [--clean]"
        ));
    }

    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("failed to create {}: {e}", data_dir.display()))?;

    if clean {
        for suffix in ["halation.db", "halation.db-wal", "halation.db-shm"] {
            let p = data_dir.join(suffix);
            if p.exists() {
                std::fs::remove_file(&p)
                    .map_err(|e| format!("failed to remove {}: {e}", p.display()))?;
                println!("Removed {}.", p.display());
            }
        }
        sweep_temp_test_dbs()?;
    }

    println!(
        "SQLite data directory is ready at {}.",
        data_dir.display()
    );
    println!("No container needed — the app creates and migrates data/halation.db on `cargo run`.");
    Ok(())
}

fn sweep_temp_test_dbs() -> Result<(), String> {
    let tmp = std::env::temp_dir();
    let entries =
        std::fs::read_dir(&tmp).map_err(|e| format!("failed to read {}: {e}", tmp.display()))?;
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("halation-test-") || name.starts_with("halation-bootstrap-") {
            let path = entry.path();
            let res = if path.is_dir() {
                std::fs::remove_dir_all(&path)
            } else {
                std::fs::remove_file(&path)
            };
            if res.is_ok() {
                removed += 1;
            }
        }
    }
    if removed == 0 {
        println!("No leftover temp test databases to remove.");
    } else {
        println!("Removed {removed} leftover temp test database path(s).");
    }
    Ok(())
}
