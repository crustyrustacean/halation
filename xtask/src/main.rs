//! `xtask` — the dev-side automation for Halation.
//!
//! One binary, one dialect, every platform. The point is that the release and
//! deploy mechanics are commands you can run without remembering the ritual,
//! and without a shell that only exists on one machine.
//!
//! Run `cargo xtask` for usage.
//!
//! Deliberately dependency-free. This is a build-time tool; every dependency
//! is another way `cargo check` at the repo root can fail for reasons that
//! have nothing to do with the app.

use std::path::PathBuf;

mod deploy;
mod devdb;
mod e2e;
mod release;

const USAGE: &str = "\
xtask — dev automation for Halation

USAGE:
    cargo xtask <command> [options]

COMMANDS:
    release-check          Verify the release bookkeeping is coherent.
                           Exits non-zero on any problem. Run this before
                           committing and after a merge.
    bump <version>         Set the version in Cargo.toml and open a new
                           CHANGELOG section for it (e.g. `bump 0.14.0`).
                           Does NOT commit or tag — that is your call.
    dev-db [--clean]       Ensure the dev Postgres container and database
                           exist. --clean also drops throwaway UUID-named
                           databases left behind by `cargo test`.
    deploy [--dry-run]     Print (or run) the droplet deploy. Refuses to touch
                           production without --yes.
    e2e [--url <url>]     Drive a real browser against the app and click the
    [--db <name>]         feed's load-more button. Starts the app itself unless
    [--keep]               --url points at a running instance. Exits non-zero
                           if the click does nothing — the one failure a
                           server-side test cannot see.
    help                   This message.

ENVIRONMENT:
    All commands run from the repository root and read the same layered config
    the app does (configuration/*.yaml, then APP_* environment variables).
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        println!("{USAGE}");
        std::process::exit(2);
    }

    let command = args[0].as_str();
    let rest = &args[1..];
    let result = match command {
        "release-check" => release::check(&repo_root()),
        "bump" => release::bump(&repo_root(), rest),
        "dev-db" => devdb::run(&repo_root(), rest),
        "deploy" => deploy::run(&repo_root(), rest),
        "e2e" => e2e::run(&repo_root(), rest),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}\n");
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };

    if let Err(err) = result {
        eprintln!("\nerror: {err}");
        std::process::exit(1);
    }
}

/// The repository root: the parent of the directory holding this crate.
///
/// xtask is always invoked through cargo from the root, but resolve this
/// explicitly rather than trusting the working directory — a wrong guess here
/// would rewrite the wrong files.
fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .expect("xtask must live one level below the repository root")
        .to_path_buf()
}
