#![allow(clippy::print_stdout)]

use std::env::{self, VarError};
use std::path::Path;
use std::process::Command;

const COMMIT_ENV: &str = "OPENPR_BUILD_GIT_COMMIT";
const DIRTY_ENV: &str = "OPENPR_BUILD_GIT_DIRTY";
const COMMITTER_DATE_ENV: &str = "OPENPR_BUILD_GIT_COMMITTER_DATE";

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed={COMMIT_ENV}");
    println!("cargo:rerun-if-env-changed={DIRTY_ENV}");
    println!("cargo:rerun-if-env-changed={COMMITTER_DATE_ENV}");

    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    // The metadata describes the entire Git work tree, including untracked files.
    // A tracked-file list cannot express that dependency. Cargo recursively scans
    // a directory input, so one repository-root dependency observes tracked and
    // untracked changes while a converged, unchanged build remains fresh.
    println!("cargo:rerun-if-changed={}", repo.display());

    let explicit_commit = optional_env(COMMIT_ENV)?;
    let explicit_dirty = optional_env(DIRTY_ENV)?;
    let explicit_committer_date = optional_env(COMMITTER_DATE_ENV)?;
    let (commit, dirty, committer_date, source) = match (explicit_commit, explicit_dirty, explicit_committer_date) {
        (None, None, None) => match git_metadata(&repo) {
            Ok(metadata) => metadata,
            Err(reason) => {
                println!("cargo:warning=build provenance is unknown: {reason}");
                unknown_metadata()
            }
        },
        (Some(commit), Some(dirty), Some(committer_date))
            if commit == "unknown" && dirty == "unknown" && committer_date == "unknown" =>
        {
            unknown_metadata()
        }
        (Some(commit), Some(dirty), Some(committer_date)) => {
            validate_commit(&commit)?;
            validate_dirty(&dirty)?;
            validate_committer_date(&committer_date)?;
            (commit, dirty, committer_date, "environment")
        }
        _ => {
            return Err(format!(
                "{COMMIT_ENV}, {DIRTY_ENV}, and {COMMITTER_DATE_ENV} must be supplied together"
            ));
        }
    };

    println!("cargo:rustc-env=OPENPR_EMBEDDED_GIT_COMMIT={commit}");
    println!("cargo:rustc-env=OPENPR_EMBEDDED_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=OPENPR_EMBEDDED_GIT_COMMITTER_DATE={committer_date}");
    println!("cargo:rustc-env=OPENPR_EMBEDDED_PROVENANCE_SOURCE={source}");
    Ok(())
}

fn optional_env(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(format!("{name} is not valid Unicode")),
    }
}

fn unknown_metadata() -> (String, String, String, &'static str) {
    (
        "unknown".to_owned(),
        "unknown".to_owned(),
        "unknown".to_owned(),
        "unknown",
    )
}

fn git_metadata(repo: &Path) -> Result<(String, String, String, &'static str), String> {
    let commit = git(repo, &["rev-parse", "HEAD"])?;
    if !is_commit(&commit) {
        return Err(format!("git returned malformed HEAD {commit:?}"));
    }
    let committer_date = git(repo, &["show", "-s", "--format=%cI", "HEAD"])?;
    validate_committer_date(&committer_date)?;
    let status = git(repo, &["status", "--porcelain", "--untracked-files=normal"])?;
    let dirty = if status.is_empty() { "false" } else { "true" };
    Ok((commit, dirty.to_owned(), committer_date, "git"))
}

fn git(repo: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        // Provenance queries must be observational. In particular, `git status`
        // may otherwise refresh .git/index, which would retrigger the repository
        // directory dependency on the next Cargo invocation.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|error| format!("could not execute git: {error}"))?;
    if !output.status.success() {
        return Err(format!("git {} exited {}", args.join(" "), output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn validate_commit(value: &str) -> Result<(), String> {
    if is_commit(value) {
        Ok(())
    } else {
        Err(format!("{COMMIT_ENV} must be a lowercase 40-hex commit, got {value:?}"))
    }
}

fn is_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_dirty(value: &str) -> Result<(), String> {
    if matches!(value, "true" | "false") {
        Ok(())
    } else {
        Err(format!("{DIRTY_ENV} must be true or false, got {value:?}"))
    }
}

fn validate_committer_date(value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    let digits =
        |range: std::ops::Range<usize>| bytes.get(range).is_some_and(|part| part.iter().all(u8::is_ascii_digit));
    let shape_ok = value.len() == 25
        && digits(0..4)
        && bytes.get(4) == Some(&b'-')
        && digits(5..7)
        && bytes.get(7) == Some(&b'-')
        && digits(8..10)
        && bytes.get(10) == Some(&b'T')
        && digits(11..13)
        && bytes.get(13) == Some(&b':')
        && digits(14..16)
        && bytes.get(16) == Some(&b':')
        && digits(17..19)
        && matches!(bytes.get(19), Some(b'+' | b'-'))
        && digits(20..22)
        && bytes.get(22) == Some(&b':')
        && digits(23..25);
    if shape_ok {
        Ok(())
    } else {
        Err(format!(
            "{COMMITTER_DATE_ENV} must be git's strict ISO 8601 committer date (%cI), got {value:?}"
        ))
    }
}
