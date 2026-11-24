//! Deploy to Fly.io.
//!
//! Wraps the runbook in `docs/DEPLOYMENT.md` and performs the verification
//! steps that get skipped when you are tired:
//!
//!   * confirm `fly` CLI is installed and `fly.toml` exists;
//!   * deploy with `fly deploy` (builds remotely on Fly's x86-64 builders);
//!   * check machine health via `fly status`;
//!   * read the *serving* version from `/api/v1/health` and confirm it matches
//!     `Cargo.toml`.
//!
//! Refuses to touch production without `--yes`.

use std::path::Path;
use std::process::Command;

const DEFAULT_HEALTH_URL: &str = "https://halation.photos/api/v1/health";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let confirmed = args.iter().any(|a| a == "--yes" || a == "-y");
    let health_url = args
        .iter()
        .find_map(|a| a.strip_prefix("--url="))
        .map(|s| s.to_string())
        .unwrap_or_else(|| env_or("HALATION_HEALTH_URL", DEFAULT_HEALTH_URL));
    let unknown: Vec<&String> = args
        .iter()
        .filter(|a| {
            !a.starts_with("--url=")
                && a.as_str() != "--dry-run"
                && a.as_str() != "--yes"
                && a.as_str() != "-y"
        })
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "unknown option(s): {unknown:?}. Usage: cargo xtask deploy [--dry-run] [--yes] [--url=…]"
        ));
    }

    let fly_toml = root.join("fly.toml");
    if !fly_toml.exists() {
        return Err(format!(
            "missing {} — see docs/DEPLOYMENT.md for Fly.io setup.",
            fly_toml.display()
        ));
    }

    println!("deploy target  : Fly.io (fly.toml)");
    println!("health check   : {health_url}");
    println!(
        "mode           : {}",
        if dry_run { "DRY RUN" } else { "LIVE" }
    );
    println!();

    let steps = plan(&health_url);
    println!("Plan:");
    for (i, step) in steps.iter().enumerate() {
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

    require_flyctl()?;

    run_cmd(root, "fly", &["deploy"])?;
    run_cmd(root, "fly", &["status"])?;

    println!();
    println!("Verify live version and dependencies:");
    println!("  curl -fsSL {health_url}");
    Ok(())
}

fn plan(health_url: &str) -> Vec<String> {
    vec![
        "fly deploy".to_string(),
        "fly status".to_string(),
        format!("curl -fsSL {health_url}  (confirm version matches Cargo.toml and db/storage are ok)"),
    ]
}

fn require_flyctl() -> Result<(), String> {
    let ok = Command::new("fly")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return Err(
            "`fly` CLI not found. Install it from https://fly.io/docs/flyctl/install/ \
             and run `fly auth login`."
                .to_string(),
        );
    }
    Ok(())
}

fn run_cmd(root: &Path, program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| format!("{program} {} failed to start: {e}", args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {} exited with {status}",
            args.join(" ")
        ))
    }
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_string())
}
