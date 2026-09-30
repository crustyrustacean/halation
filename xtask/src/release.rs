//! Release bookkeeping: keep `Cargo.toml`, `CHANGELOG.md`, and the git history
//! telling the same story.
//!
//! This exists because the bookkeeping has already slipped once: the storage
//! partitioning merge shipped without its `Cargo.toml` version bump and had to
//! be folded into a later release. Checking is cheap; remembering is not.

use std::path::Path;
use std::process::Command;

/// A `## [version] - date` heading.
#[derive(Debug, PartialEq, Eq)]
struct Entry {
    version: String,
    date: String,
    line: usize,
}

pub fn check(root: &Path) -> Result<(), String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("could not read Cargo.toml: {e}"))?;
    let changelog = std::fs::read_to_string(root.join("CHANGELOG.md"))
        .map_err(|e| format!("could not read CHANGELOG.md: {e}"))?;

    let cargo_version = manifest_version(&manifest)?;
    let entries = parse_entries(&changelog);
    let mut problems: Vec<String> = Vec::new();

    println!("cargo version: {cargo_version}");
    println!(
        "changelog:     {} entries, newest {} ({})",
        entries.len(),
        newest_label(&entries),
        newest_date(&entries)
    );
    println!();

    if entries.is_empty() {
        problems.push("CHANGELOG.md has no `## [version] - date` entries".to_string());
    } else {
        if entries[0].version != cargo_version {
            problems.push(format!(
                "Cargo.toml says {cargo_version} but the newest CHANGELOG entry is {} — \
                 run `cargo xtask bump {}` (or fix the changelog by hand)",
                entries[0].version,
                next_patch(&cargo_version)
            ));
        }

        // The changelog descends by version, so its dates must descend too. A
        // release cannot ship before the one that precedes it.
        let mut previous: Option<&Entry> = None;
        for entry in &entries {
            if let Some(prev) = previous
                && entry.date > prev.date
            {
                problems.push(format!(
                    "line {}: [{}] is dated {} but the entry above it [{}] is dated {} — \
                     releases are newest-first, so this date goes backwards",
                    entry.line, entry.version, entry.date, prev.version, prev.date
                ));
            }
            previous = Some(entry);
        }

        // Duplicate versions mean a release was edited rather than appended.
        for (i, entry) in entries.iter().enumerate() {
            if entries[..i].iter().any(|e| e.version == entry.version) {
                problems.push(format!(
                    "line {}: version {} appears more than once",
                    entry.line, entry.version
                ));
            }
        }
    }

    if let Some(problem) = check_version_bump(root, &cargo_version) {
        problems.push(problem);
    }

    if problems.is_empty() {
        println!("release-check: OK — Cargo.toml, CHANGELOG.md, and git history agree.");
        return Ok(());
    }

    println!("release-check: {} problem(s) found:", problems.len());
    for problem in &problems {
        println!("  - {problem}");
    }
    Err("release bookkeeping is not coherent (see above)".to_string())
}

/// Flag a version/tag disagreement — the miss that cost a day when the storage
/// partitioning merge shipped without its `Cargo.toml` version bump.
///
/// Returns `Some(problem)` only when a disagreement is real; `None` when there
/// is nothing to compare against (no tags yet) or everything agrees.
fn check_version_bump(root: &Path, current: &str) -> Option<String> {
    if !root.join(".git").exists() {
        return None;
    }
    let tagged = latest_tag(root)?;
    if tagged == current {
        return None;
    }
    Some(format!(
        "latest git tag is {tagged} but Cargo.toml says {current} — they should match"
    ))
}

fn latest_tag(root: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
mod check_tests {
    use super::*;

    /// The exact shape that produced the false alarm during the xtask build:
    /// 0.13.0 is dated 2026-09-24 and 0.12.0 was released the day before.
    #[test]
    fn one_real_inversion_is_reported_not_four() {
        let changelog = concat!(
            "## [0.13.0] - 2026-09-24\n\n## [0.12.0] - 2026-10-01\n",
            "## [0.11.0] - 2026-09-22\n\n## [0.10.0] - 2026-09-21\n",
        );
        let entries = parse_entries(changelog);
        let mut problems = Vec::new();
        let mut previous: Option<&Entry> = None;
        for entry in &entries {
            if let Some(prev) = previous
                && entry.date > prev.date
            {
                problems.push(entry.version.clone());
            }
            previous = Some(entry);
        }
        // Only 0.12.0 is genuinely wrong; 0.11.0 and 0.10.0 are correctly ordered.
        assert_eq!(vec!["0.12.0".to_string()], problems);
    }
}

pub fn bump(root: &Path, args: &[String]) -> Result<(), String> {
    let version = args
        .first()
        .ok_or_else(|| "usage: cargo xtask bump <version> (e.g. 0.14.0)".to_string())?;
    validate_version(version)?;

    let manifest_path = root.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("could not read Cargo.toml: {e}"))?;
    let old = manifest_version(&manifest)?;

    let updated = set_manifest_version(&manifest, version)?;
    std::fs::write(&manifest_path, &updated)
        .map_err(|e| format!("could not write Cargo.toml: {e}"))?;

    let today = today();
    let changelog_path = root.join("CHANGELOG.md");
    let changelog = std::fs::read_to_string(&changelog_path)
        .map_err(|e| format!("could not read CHANGELOG.md: {e}"))?;
    let with_section = open_changelog_section(&changelog, version, &today);
    std::fs::write(&changelog_path, &with_section)
        .map_err(|e| format!("could not write CHANGELOG.md: {e}"))?;

    println!("bumped {old} -> {version}");
    println!("  Cargo.toml  version = \"{version}\"");
    println!("  CHANGELOG.md opened a [{}] - {today} section", version);
    println!();
    println!("Write the entry, then:");
    println!("  cargo xtask release-check");
    println!("  git commit -am \"chore: release {version}\"");
    Ok(())
}

fn manifest_version(manifest: &str) -> Result<String, String> {
    manifest
        .lines()
        .find_map(|line| {
            let rest = line.trim().strip_prefix("version")?;
            let rest = rest.trim_start().strip_prefix('=')?.trim();
            let value = rest.trim_matches('"');
            // The workspace member `xtask` has its own version; only the first
            // `version` in the root manifest is the app's.
            if value.chars().all(|c| c.is_ascii_digit() || c == '.') {
                Some(value.to_string())
            } else {
                None
            }
        })
        .ok_or_else(|| "could not find a `version = \"…\"` in Cargo.toml".to_string())
}

fn set_manifest_version(manifest: &str, version: &str) -> Result<String, String> {
    let mut replaced = false;
    let mut out = String::with_capacity(manifest.len());
    for line in manifest.lines() {
        let trimmed = line.trim();
        let is_version_line = trimmed.starts_with("version")
            && trimmed
                .trim_start_matches("version")
                .trim_start()
                .starts_with('=')
            && !replaced;
        if is_version_line {
            let indent = &line[..line.len() - line.trim_start().len()];
            out.push_str(&format!("{indent}version = \"{version}\"\n"));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        return Err("could not find the version line in Cargo.toml".to_string());
    }
    Ok(out)
}

fn parse_entries(changelog: &str) -> Vec<Entry> {
    changelog
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let rest = line.trim().strip_prefix("## [")?;
            let (version, date) = rest.split_once(']')?;
            Some(Entry {
                version: version.trim().to_string(),
                date: date
                    .trim()
                    .trim_start_matches(['-', ' '])
                    .trim()
                    .to_string(),
                line: index + 1,
            })
        })
        .collect()
}

fn newest_label(entries: &[Entry]) -> String {
    entries
        .first()
        .map(|e| e.version.clone())
        .unwrap_or_else(|| "-".to_string())
}

fn newest_date(entries: &[Entry]) -> String {
    entries
        .first()
        .map(|e| e.date.clone())
        .unwrap_or_else(|| "-".to_string())
}

fn open_changelog_section(changelog: &str, version: &str, date: &str) -> String {
    let heading = format!("## [{version}] - {date}");
    let preamble_end = changelog.find("## [").unwrap_or(changelog.len());
    let (head, tail) = changelog.split_at(preamble_end);
    let body = "# Write the notable changes for this release here.\n\n";
    format!("{head}{heading}\n\n{body}{tail}")
}

fn validate_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || !version.chars().all(|c| c.is_ascii_digit() || c == '.')
        || version.starts_with('.')
        || version.ends_with('.')
    {
        return Err(format!("`{version}` is not a version like 0.14.0 or 1.0.0"));
    }
    if version.split('.').count() < 2 {
        return Err(format!("`{version}` needs at least major.minor"));
    }
    Ok(())
}

fn next_patch(version: &str) -> String {
    let mut parts: Vec<u64> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    while parts.len() < 3 {
        parts.push(0);
    }
    parts[2] += 1;
    parts
        .iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// Today's date as `YYYY-MM-DD`.
///
/// No `chrono` in xtask on purpose, so ask the OS — but portably. The
/// earlier version ran `cmd /C date`, which simply does not exist on
/// Linux or macOS, so it fell through to the placeholder and wrote
/// `## [x.y.z] - YYYY-MM-DD` into the changelog.
fn today() -> String {
    #[cfg(windows)]
    let candidates: Vec<Vec<String>> = vec![
        // Locale-independent ISO date; not affected by the user's
        // regional date format (dd/mm/yyyy vs mm/dd/yyyy).
        vec![
            "powershell".into(),
            "-NoProfile".into(),
            "-Command".into(),
            "Get-Date -Format yyyy-MM-dd".into(),
        ],
    ];

    #[cfg(not(windows))]
    let candidates: Vec<Vec<String>> = vec![
        vec!["date".into(), "+%Y-%m-%d".into()],
        vec![
            "python3".into(),
            "-c".into(),
            "import datetime;print(datetime.date.today().isoformat())".into(),
        ],
    ];

    for args in candidates {
        if let Ok(output) = Command::new(&args[0]).args(&args[1..]).output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if looks_like_iso_date(&text) {
                return text;
            }
        }
    }

    // Last resort: day-precision seconds from the epoch. Correct to the
    // day (not to the second), which is all a changelog needs.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (now / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Reject a shell's chatter (locale warnings, extra lines) before trusting
/// its output as a date.
fn looks_like_iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

/// Days since 1970-01-01 to a civil `(year, month, day)`. Howard Hinnant's
/// `civil_from_days`, which is exact for the whole proleptic Gregorian
/// range — no lookup table, no leap-year special cases.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod today_tests {
    use super::*;

    #[test]
    fn today_is_iso_shaped() {
        let text = today();
        assert!(looks_like_iso_date(&text), "got {text:?}");
    }

    #[test]
    fn rejects_shell_chatter() {
        assert!(!looks_like_iso_date(""));
        assert!(!looks_like_iso_date("2026-9-24"));
        assert!(!looks_like_iso_date("Wed Sep 24 18:00:00 UTC 2026"));
        assert!(looks_like_iso_date("2026-09-24"));
    }

    #[test]
    fn civil_conversion_matches_known_dates() {
        // Epoch, a leap day and the day after it (the off-by-one boundary),
        // a century-leap boundary, and the 2026 date this project ships.
        assert_eq!((1970, 1, 1), civil_from_days(0));
        assert_eq!((2024, 2, 29), civil_from_days(19_782));
        assert_eq!((2024, 3, 1), civil_from_days(19_783));
        assert_eq!((2000, 3, 1), civil_from_days(11_017));
        assert_eq!((2026, 9, 24), civil_from_days(20_720));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_app_version_not_the_workspace_member() {
        let manifest = "[package]\nname = \"halation\"\nversion = \"0.13.0\"\nedition = \"2024\"\n\n[workspace]\nmembers = [\"xtask\"]\n";
        assert_eq!("0.13.0", manifest_version(manifest).unwrap());
    }

    #[test]
    fn rewrites_only_the_app_version() {
        let manifest = "[package]\nname = \"halation\"\nversion = \"0.13.0\"\n\n[workspace]\nmembers = [\"xtask\"]\n";
        let out = set_manifest_version(manifest, "0.14.0").unwrap();
        assert!(out.contains("version = \"0.14.0\""));
        assert_eq!(1, out.matches("version = ").count());
    }

    #[test]
    fn parses_entries_with_lines() {
        let changelog =
            "# Changelog\n\n## [0.14.0] - 2026-09-29\n\nstuff\n\n## [0.13.0] - 2026-09-24\n";
        let entries = parse_entries(changelog);
        assert_eq!(2, entries.len());
        assert_eq!("0.14.0", entries[0].version);
        assert_eq!("2026-09-29", entries[0].date);
        assert_eq!("0.13.0", entries[1].version);
    }

    #[test]
    fn detects_a_date_inversion() {
        // Newest first, so a date that increases going down is a real problem.
        let changelog = "## [0.13.0] - 2026-09-24\n\n## [0.12.0] - 2026-10-01\n";
        let entries = parse_entries(changelog);
        let prev = &entries[0];
        let cur = &entries[1];
        assert!(cur.date > prev.date, "expected 0.12.0 to be flagged");
    }

    #[test]
    fn ordered_dates_are_fine() {
        let changelog = "## [0.13.0] - 2026-09-24\n\n## [0.12.0] - 2026-09-23\n";
        let entries = parse_entries(changelog);
        assert!(entries[1].date <= entries[0].date);
    }

    #[test]
    fn validates_versions() {
        assert!(validate_version("0.14.0").is_ok());
        assert!(validate_version("1.0").is_ok());
        assert!(validate_version("").is_err());
        assert!(validate_version("0.14.0-rc1").is_err());
        assert!(validate_version("banana").is_err());
        assert!(validate_version("0.14.").is_err());
    }

    #[test]
    fn bumps_the_patch_component() {
        // next_patch is a mechanical patch-level bump, not semver-aware:
        // whether a change warrants 0.13.1 or 0.14.0 is a human judgement.
        assert_eq!("0.13.1", next_patch("0.13.0"));
        assert_eq!("1.0.1", next_patch("1.0.0"));
    }
}
