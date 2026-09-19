use anyhow::Result;
use colored::Colorize;
use std::fs;

use crate::runner::{
    FUZZ_DIR, RunOptions, find_repo_root, print_section, print_step, print_success, run_command,
};

pub fn run_clean(all: bool, dry_run: bool) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Workspace Clean");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    if !dry_run {
        print_step("Running cargo clean...");
        run_command("cargo", &["clean"], &opts)?;
    } else {
        println!("{} Would run cargo clean", "•".yellow());
    }

    let cleanup_targets = [
        repo_root.join(".shuck_cache"),
        repo_root.join(".cache/profiles"),
        repo_root.join("target/large-corpus-report"),
        repo_root.join("target/npm"),
        repo_root.join("target/wasm-test"),
        repo_root.join("editors/vscode/bin"),
    ];

    for path in &cleanup_targets {
        if path.exists() {
            if dry_run {
                println!("{} Would remove: {}", "•".yellow(), path.display());
            } else {
                print_step(&format!("Removing: {}", path.display()));
                let _ = fs::remove_dir_all(path);
            }
        }
    }

    if all {
        let all_targets = [
            repo_root.join(FUZZ_DIR).join("artifacts"),
            repo_root.join(FUZZ_DIR).join("corpus"),
        ];
        for path in &all_targets {
            if path.exists() {
                if dry_run {
                    println!("{} Would remove: {}", "•".yellow(), path.display());
                } else {
                    print_step(&format!("Removing: {}", path.display()));
                    let _ = fs::remove_dir_all(path);
                }
            }
        }
    }

    print_success("Workspace cleanup complete.");
    Ok(())
}
