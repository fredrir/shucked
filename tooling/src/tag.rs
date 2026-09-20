use anyhow::{Context, Result, bail};
use regex::Regex;
use std::fs;
use std::path::Path;

use crate::runner::find_repo_root;

/// Component whose version can be read or bumped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Shucked,
    Vscode,
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Target::Shucked => "shucked",
            Target::Vscode => "vscode",
        }
    }
}

/// What to do with the selected version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Get,
    Up,
    Down,
}

pub fn run_tag(target: Option<Target>, action: Action) -> Result<()> {
    let repo_root = find_repo_root()?;
    let targets = match target {
        Some(target) => vec![target],
        None => vec![Target::Shucked, Target::Vscode],
    };

    match action {
        Action::Get => {
            for target in &targets {
                let version = read_version(&repo_root, *target)?;
                println!("{:<12} {version}", target.label());
            }
        }
        Action::Up | Action::Down => {
            let up = action == Action::Up;
            for target in &targets {
                let old = read_version(&repo_root, *target)?;
                let new = bump_patch(&old, up)?;
                write_version(&repo_root, *target, &new)?;
                if targets.len() == 1 {
                    println!("{old} --> {new}");
                } else {
                    println!("{:<12} {old} --> {new}", target.label());
                }
            }
        }
    }

    Ok(())
}

/// Increment or decrement the patch component of a `major.minor.patch` version.
fn bump_patch(version: &str, up: bool) -> Result<String> {
    let mut parts = version.split('.');
    let (Some(major), Some(minor), Some(patch), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("unsupported version format: {version}");
    };

    let major: u64 = major
        .parse()
        .with_context(|| format!("invalid major version in {version}"))?;
    let minor: u64 = minor
        .parse()
        .with_context(|| format!("invalid minor version in {version}"))?;
    let patch: u64 = patch
        .parse()
        .with_context(|| format!("invalid patch version in {version}"))?;

    let patch = if up {
        patch
            .checked_add(1)
            .with_context(|| format!("version overflow while bumping {version}"))?
    } else {
        patch
            .checked_sub(1)
            .with_context(|| format!("cannot bump {version} below zero"))?
    };

    Ok(format!("{major}.{minor}.{patch}"))
}

fn read_version(repo_root: &Path, target: Target) -> Result<String> {
    match target {
        Target::Shucked => {
            let path = repo_root.join("Cargo.toml");
            let text = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let re = Regex::new(r#"(?m)^version\s*=\s*"([^"]+)""#)?;
            let version = re
                .captures(&text)
                .and_then(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
                .with_context(|| {
                    format!("could not find workspace version in {}", path.display())
                })?;
            Ok(version)
        }
        Target::Vscode => {
            let path = repo_root.join("editors/vscode/package.json");
            let text = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let re = Regex::new(r#"(?m)^\s*"version":\s*"([^"]+)""#)?;
            let version = re
                .captures(&text)
                .and_then(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
                .with_context(|| {
                    format!("could not find extension version in {}", path.display())
                })?;
            Ok(version)
        }
    }
}

fn write_version(repo_root: &Path, target: Target, new: &str) -> Result<()> {
    match target {
        Target::Shucked => {
            // `[workspace.package] version = "x.y.z"`
            replace(
                &repo_root.join("Cargo.toml"),
                r#"(?m)^(version\s*=\s*")[^"]*(")"#,
                new,
                false,
            )?;

            // Internal crate versions in `[workspace.dependencies]`.
            replace(
                &repo_root.join("Cargo.toml"),
                r#"(?m)^(shucked-[\w-]+\s*=\s*\{[^}\n]*version\s*=\s*")[^"]*(")"#,
                new,
                true,
            )?;

            // Tooling crate version.
            replace(
                &repo_root.join("tooling/Cargo.toml"),
                r#"(?m)^(version\s*=\s*")[^"]*(")"#,
                new,
                false,
            )?;

            // Clap `version = "x.y.z"` attribute in the tooling binary.
            replace(
                &repo_root.join("tooling/src/main.rs"),
                r#"(?m)^(\s*version\s*=\s*")[^"]*(",)"#,
                new,
                false,
            )?;

            // Workspace member versions recorded in the lockfile.
            replace(
                &repo_root.join("Cargo.lock"),
                r#"(?m)(^name = "shucked-[^"]+"\nversion = ")[^"]*(")"#,
                new,
                true,
            )?;

            // release-please tracks the current released version here.
            replace(
                &repo_root.join(".release-please-manifest.json"),
                r#"(?m)^(\s*"\.":\s*")[^"]*(")"#,
                new,
                false,
            )?;
        }
        Target::Vscode => {
            replace(
                &repo_root.join("editors/vscode/package.json"),
                r#"(?m)^(\s*"version":\s*")[^"]*(",)"#,
                new,
                false,
            )?;
        }
    }

    Ok(())
}

/// Rewrite `path`, substituting `new` into the captured version group of `pattern`.
fn replace(path: &Path, pattern: &str, new: &str, all: bool) -> Result<()> {
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let re = Regex::new(pattern)?;
    let replacement = format!("${{1}}{new}${{2}}");
    let updated = if all {
        re.replace_all(&text, replacement.as_str()).to_string()
    } else {
        re.replace(&text, replacement.as_str()).to_string()
    };

    if updated != text {
        fs::write(path, updated).with_context(|| format!("failed to write {}", path.display()))?;
    }

    Ok(())
}
