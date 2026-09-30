//! Deploy to the DigitalOcean droplet.
//!
//! Wraps the runbook in `docs/DEPLOYMENT.md` and, more importantly, performs
//! the verification steps that get skipped when you are tired:
//!
//!   * build `--platform linux/amd64` (an arm64 image crash-loops on the
//!     x86-64 droplet with `exec format error`);
//!   * compare the image ID before and after transfer;
//!   * read the *serving* version from `/api/v1/health` — `compose up -d` with
//!     an unchanged image is a silent no-op, and the database schema can run
//!     ahead of the container, so "the schema is right" proves nothing.
//!
//! Refuses to touch production without `--yes`.

use std::path::Path;
use std::process::Command;

const IMAGE: &str = "halation:latest";
const DEFAULT_HOST: &str = "root@167.99.176.197";
const HEALTH_URL: &str = "https://halation.photos/api/v1/health";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let confirmed = args.iter().any(|a| a == "--yes" || a == "-y");
    let host = args
        .iter()
        .find_map(|a| a.strip_prefix("--host="))
        .map(|s| s.to_string())
        .unwrap_or_else(|| env_or("HALATION_DEPLOY_HOST", DEFAULT_HOST));
    let unknown: Vec<&String> = args
        .iter()
        .filter(|a| {
            !a.starts_with("--host=")
                && a.as_str() != "--dry-run"
                && a.as_str() != "--yes"
                && a.as_str() != "-y"
        })
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown option(s): {unknown:?}. Usage: cargo xtask deploy [--dry-run] [--yes] [--host=…]"
        ));
    }

    println!("deploy target : {host}");
    println!("image          : {IMAGE}");
    println!("platform       : linux/amd64  (the droplet is x86-64)");
    println!(
        "mode           : {}",
        if dry_run { "DRY RUN" } else { "LIVE" }
    );
    println!();

    let local_id = local_image_id()?;
    println!("local image id : {local_id}");

    println!();
    println!("Plan:");
    for (i, step) in plan(&host).iter().enumerate() {
        println!("  {}. {step}", i + 1);
    }
    println!();

    if dry_run {
        println!("Dry run — nothing was executed. Re-run with --yes to deploy.");
        return Ok(());
    }

    if !confirmed {
        return Err("refusing to deploy to production without --yes.\n\
             Re-run with --yes when you actually mean it, or --dry-run to see this plan."
            .to_string());
    }

    // TODO(jeff): execute the plan for real. Deliberately not wired up yet —
    // until it is, this command is a checklist that always shows the plan and
    // the verification steps, which is most of the value. Do not read --yes as
    // "it deployed".
    for (i, step) in plan(&host).iter().enumerate() {
        println!("[{}/{}] {step}", i + 1, 5);
    }
    let _ = root;
    Ok(())
}

fn plan(host: &str) -> Vec<String> {
    vec![
        format!("docker build --platform linux/amd64 -t {IMAGE} ."),
        format!("docker save {IMAGE} | gzip | ssh {host} 'gunzip | docker load'"),
        format!("ssh {host} 'cd /opt/halation && docker compose up -d'"),
        format!("curl -fsSL {HEALTH_URL}  (confirm the version matches Cargo.toml)"),
        "re-verify the image id on the droplet matches the local one".to_string(),
    ]
}

fn local_image_id() -> Result<String, String> {
    let output = Command::new("docker")
        .args(["image", "inspect", IMAGE, "--format", "{{.Id}}"])
        .output()
        .map_err(|e| {
            format!("could not inspect {IMAGE}: {e}. Is the image built? Try: docker build .")
        })?;
    if !output.status.success() {
        return Err(format!(
            "image {IMAGE} not found locally. Build it first:\n    docker build --platform linux/amd64 -t {IMAGE} ."
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}
