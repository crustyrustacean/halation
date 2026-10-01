//! Dev database bootstrap.
//!
//! Absorbs `scripts/init_dev_db.sh` and `scripts/init_db.ps1` so the dev
//! database has one implementation on every platform. The shell pair had
//! drifted into a genuine footgun: a `.ps1` with no `param()` block silently
//! discarded `-Clean`, exiting 0 having done nothing.
//!
//! The app applies its own migrations on boot; this deliberately does not.

use std::path::Path;
use std::process::{Command, Stdio};

const CONTAINER: &str = "halation-db-1";
const DB_USER: &str = "postgres";
const DB_PASSWORD: &str = "password";
const DB_PORT: &str = "5433";
const DB_NAME: &str = "halation";

/// Resolved configuration. The shell scripts this replaces were
/// env-overridable (`CONTAINER`, `DB_USER`, `DB_PASSWORD`, `DB_PORT`,
/// `DB_NAME`), and so is this — hardcoding the defaults silently ignored
/// anyone (or CI) that had set them.
struct DevDb {
    container: String,
    user: String,
    password: String,
    port: String,
    name: String,
}

impl DevDb {
    fn from_env() -> Self {
        Self {
            container: env_or("CONTAINER", CONTAINER),
            user: env_or("DB_USER", DB_USER),
            password: env_or("DB_PASSWORD", DB_PASSWORD),
            port: env_or("DB_PORT", DB_PORT),
            name: env_or("DB_NAME", DB_NAME),
        }
    }
}

fn env_or(key: &str, fallback: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => fallback.to_string(),
    }
}

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

    require_docker()?;

    let db = DevDb::from_env();

    // Order matters: the sweep queries through the container, so the
    // container has to exist first. Sweeping before `ensure_container`
    // made `--clean` fail outright on a fresh machine.
    ensure_container(&db)?;

    if !wait_until_ready(&db)? {
        return Err(format!(
            "Postgres did not become ready in time. Check `docker logs {}`.",
            db.container
        ));
    }

    if clean {
        sweep_test_databases(&db)?;
    }

    let exists = psql(
        &db,
        &[
            "-tAc",
            &format!("SELECT 1 FROM pg_database WHERE datname = '{}'", db.name),
        ],
    )?;
    if exists.trim() != "1" {
        psql(&db, &["-c", &format!("CREATE DATABASE {}", db.name)])?;
        println!("Created database '{}'.", db.name);
    }

    println!("Database '{}' is ready on localhost:{}.", db.name, db.port);
    println!("The app applies migrations on boot — just `cargo run`.");
    let _ = root;
    Ok(())
}

fn require_docker() -> Result<(), String> {
    let ok = Command::new("docker")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return Err(
            "docker is not responding. Install Docker Desktop and make sure it is running."
                .to_string(),
        );
    }
    Ok(())
}

fn container_state(db: &DevDb) -> Result<String, String> {
    let out = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name=^/{}$", db.container),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .map_err(|e| format!("docker ps failed: {e}"))?;
    let running = Command::new("docker")
        .args([
            "ps",
            "--filter",
            &format!("name=^/{}$", db.container),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .map_err(|e| format!("docker ps failed: {e}"))?;

    let all = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let live = String::from_utf8_lossy(&running.stdout).trim().to_string();
    if live == db.container {
        Ok("running".to_string())
    } else if all == db.container {
        Ok("stopped".to_string())
    } else {
        Ok("missing".to_string())
    }
}

fn ensure_container(db: &DevDb) -> Result<(), String> {
    match container_state(db)? {
        state if state == "running" => {
            println!("Container '{}' is running.", db.container);
            ensure_restart_policy(db)
        }
        state if state == "stopped" => {
            println!("Starting existing container '{}'...", db.container);
            run_docker(&["start", &db.container])
        }
        _ => {
            println!("Creating container '{}'...", db.container);
            run_docker(&[
                "run",
                "-d",
                "--name",
                &db.container,
                // Survive Docker Desktop quitting and restarting. Without
                // this the dev database stops every time the daemon does,
                // and the next `cargo test` fails at connect — which looks
                // like a test failure rather than a stopped container.
                "--restart",
                "unless-stopped",
                "-e",
                &format!("POSTGRES_USER={}", db.user),
                "-e",
                &format!("POSTGRES_PASSWORD={}", db.password),
                "--health-cmd=pg_isready -U postgres",
                "--health-interval=2s",
                "--health-timeout=3s",
                "--health-retries=10",
                "-p",
                &format!("{}:5432", db.port),
                "postgres:17",
                // The real flag. `postgres -N 1000` is silently ignored by the
                // image, so the connection limit was never actually raised.
                "-c",
                "max_connections=1000",
            ])
        }
    }
}

/// Bring an already-created container up to the current restart policy.
///
/// `--restart` cannot be changed on an existing container, so a container
/// created before this was added keeps its old policy forever. This updates
/// it in place, and is a no-op once the policy already matches.
fn ensure_restart_policy(db: &DevDb) -> Result<(), String> {
    let current = std::process::Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{.HostConfig.RestartPolicy.Name}}",
            &db.container,
        ])
        .output()
        .map_err(|e| format!("could not inspect the container: {e}"))?;
    if !current.status.success() {
        return Ok(());
    }
    let policy = String::from_utf8_lossy(&current.stdout).trim().to_string();
    if policy == "unless-stopped" {
        return Ok(());
    }
    println!(
        "Updating restart policy on '{}' from '{}' to 'unless-stopped'.",
        db.container, policy
    );
    run_docker(&["update", "--restart", "unless-stopped", &db.container])
}

/// Probe with a real query over TCP.
///
/// `pg_isready` on the container's unix socket is a trap: during `initdb` the
/// entrypoint runs a temporary socket-only server that answers "ready", then
/// goes away. Querying over TCP is both race-free and the path the app uses.
fn postgres_ready(db: &DevDb) -> bool {
    Command::new("docker")
        .args([
            "exec",
            "-e",
            &format!("PGPASSWORD={}", db.password),
            &db.container,
            "psql",
            "-U",
            &db.user,
            "-h",
            "127.0.0.1",
            "-tAc",
            "SELECT 1",
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn wait_until_ready(db: &DevDb) -> Result<bool, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if postgres_ready(db) {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        println!("Waiting for Postgres...");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Run psql inside the container and return stdout.
///
/// Deliberately no `-i`: every call passes its SQL with `-c`, so stdin is
/// never used. Combining `-i` with `output()` is a deadlock — the child
/// inherits a stdin pipe that nothing writes to or closes, and on a chatty
/// statement (`DROP DATABASE` emits a NOTICE per backend) it blocks filling
/// the stdout pipe while the parent waits for exit.
fn psql(db: &DevDb, args: &[&str]) -> Result<String, String> {
    let output = Command::new("docker")
        .args([
            "exec",
            "-e",
            &format!("PGPASSWORD={}", db.password),
            &db.container,
            "psql",
            "-U",
            &db.user,
            "-h",
            "127.0.0.1",
        ])
        .args(args)
        // Belt and braces: even if a future call adds `-i`, nothing may be
        // left blocking on a pipe we never write to.
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("docker exec psql failed: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "psql failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    // A zero-row query prints nothing; that arrives here as an empty string,
    // which is what the caller's comparison expects.
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn sweep_test_databases(db: &DevDb) -> Result<(), String> {
    println!("Dropping throwaway test databases...");
    let listing = psql(
        db,
        &[
            "-tAc",
            "SELECT datname FROM pg_database WHERE datname ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'",
        ],
    )?;
    let mut dropped = 0;
    for line in listing.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        psql(
            db,
            &[
                "-c",
                &format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"),
            ],
        )?;
        dropped += 1;
    }
    if dropped == 0 {
        println!("No throwaway test databases to drop.");
    } else {
        println!("Dropped {dropped} test database(s).");
    }
    Ok(())
}

fn run_docker(args: &[&str]) -> Result<(), String> {
    let status = Command::new("docker")
        .args(args)
        .status()
        .map_err(|e| format!("docker {} failed: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("docker {} failed with {status}", args.join(" ")))
    }
}
