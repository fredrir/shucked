use anyhow::{Context, Result, bail};
use colored::Colorize;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::runner::{
    find_repo_root, print_error, print_section, print_step, print_success, print_warning,
};

pub const DEFAULT_WORKFLOW_PATH: &str = ".github/workflows/release.yml";

/// Get the line indices (start, end) for a top-level job section in workflow yaml.
fn get_job_section(lines: &[String], job_name: &str) -> (Option<usize>, Option<usize>) {
    let job_re = Regex::new(&format!(r"^  {}:\s*$", regex::escape(job_name))).unwrap();
    let next_job_re = Regex::new(r"^  \w").unwrap();

    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        let stripped = line.trim_end_matches(['\r', '\n']);
        if job_re.is_match(stripped) {
            start = Some(i);
        } else if let Some(s) = start
            && i > s
            && next_job_re.is_match(line)
        {
            return (Some(s), Some(i));
        }
    }

    if let Some(s) = start {
        (Some(s), Some(lines.len()))
    } else {
        (None, None)
    }
}

/// Check workflow content for security issues.
pub fn check_security_content(content: &str) -> Vec<String> {
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut issues = Vec::new();

    // Check 1: top-level permissions must not grant write
    for (i, line) in lines.iter().enumerate() {
        if line.trim_end() == "permissions:" && !line.starts_with(' ') {
            for perm_line in lines.iter().skip(i + 1) {
                let stripped = perm_line.trim();
                if stripped.is_empty() || stripped.starts_with('#') {
                    continue;
                }
                if !perm_line.starts_with("  ") || perm_line.starts_with("    ") {
                    break;
                }
                if perm_line.contains("write") {
                    issues.push("top-level permissions grants write access".to_string());
                    break;
                }
            }
            break;
        }
    }

    // Check 2: plan job has per-job permissions
    let (start, end) = get_job_section(&lines, "plan");
    if let (Some(s), Some(e)) = (start, end) {
        let section = lines[s..e].join("\n");
        let perms_re = Regex::new(r"(?m)^\s{4}permissions:").unwrap();
        if !perms_re.is_match(&section) {
            issues.push("plan job missing per-job permissions".to_string());
        }
    }

    // Check 3: host job has per-job permissions and environment gate
    let (start, end) = get_job_section(&lines, "host");
    if let (Some(s), Some(e)) = (start, end) {
        let section = lines[s..e].join("\n");
        let perms_re = Regex::new(r"(?m)^\s{4}permissions:").unwrap();
        if !perms_re.is_match(&section) {
            issues.push("host job missing per-job permissions".to_string());
        }
        if !section.contains("environment: release") {
            issues.push("host job missing environment: release".to_string());
        }
    }

    // Check 4: release commands should not interpolate needs.* expressions directly in shell
    if content.contains("dist build ${{ needs.plan.outputs.tag-flag }}") {
        issues.push("dist build uses direct template expansion in run block".to_string());
    }
    if content.contains("dist host ${{ needs.plan.outputs.tag-flag }}") {
        issues.push("host dist command uses direct template expansion in run block".to_string());
    }
    if content.contains("gh release create \"${{ needs.plan.outputs.tag }}\"") {
        issues.push("release creation uses direct template expansion in run block".to_string());
    }

    // Check 5: Homebrew publishing uses secret and should be behind release environment
    let (start, end) = get_job_section(&lines, "publish-homebrew-formula");
    if let (Some(s), Some(e)) = (start, end) {
        let section = lines[s..e].join("\n");
        if section.contains("HOMEBREW_TAP_TOKEN") && !section.contains("environment: release") {
            issues.push("publish-homebrew-formula missing environment: release".to_string());
        }
    }

    // Check 6: global release artifacts still need cargo-cyclonedx for SBOM extra artifact
    let (start, end) = get_job_section(&lines, "build-global-artifacts");
    if let (Some(s), Some(e)) = (start, end) {
        let section = lines[s..e].join("\n");
        if !section.contains("Install cargo-cyclonedx") {
            issues.push("build-global-artifacts missing cargo-cyclonedx install".to_string());
        }
    }

    // Check 7: release publishing must handle tags that already have a GitHub Release
    if !content.contains("gh release upload \"${NEEDS_PLAN_OUTPUTS_TAG}\" artifacts/* --clobber") {
        issues.push("release publishing no longer updates existing GitHub releases".to_string());
    }

    issues
}

/// Apply security hardening fixes to release workflow content.
pub fn fix_security_content(content: &str) -> String {
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    // Fix 1: top-level permissions -> read-only
    let write_word_re = Regex::new(r"\bwrite\b").unwrap();
    for i in 0..lines.len() {
        if lines[i].trim_end() == "permissions:" && !lines[i].starts_with(' ') {
            for perm_line in lines.iter_mut().skip(i + 1) {
                let stripped = perm_line.trim();
                if stripped.is_empty() || stripped.starts_with('#') {
                    continue;
                }
                if !perm_line.starts_with("  ") || perm_line.starts_with("    ") {
                    break;
                }
                if perm_line.contains("write") {
                    *perm_line = write_word_re.replace(perm_line, "read").to_string();
                }
            }
            break;
        }
    }

    // Fix 2: plan job - insert per-job permissions after runs-on
    let (start, end) = get_job_section(&lines, "plan");
    if let (Some(s), Some(e)) = (start, end) {
        let section_text = lines[s..e].join("\n");
        let perms_re = Regex::new(r"(?m)^\s{4}permissions:").unwrap();
        if !perms_re.is_match(&section_text) {
            for i in s..e {
                if lines[i].trim().starts_with("runs-on:") {
                    lines.insert(i + 1, "    permissions:".to_string());
                    lines.insert(i + 2, "      contents: write".to_string());
                    break;
                }
            }
        }
    }

    // Fix 3: host job - insert per-job permissions + environment before env:
    let (start, end) = get_job_section(&lines, "host");
    if let (Some(s), Some(e)) = (start, end) {
        let section_text = lines[s..e].join("\n");
        let perms_re = Regex::new(r"(?m)^\s{4}permissions:").unwrap();
        let has_perms = perms_re.is_match(&section_text);
        let has_env = section_text.contains("environment: release");

        if !has_perms || !has_env {
            for i in s..e {
                if lines[i].trim().starts_with("env:") && lines[i].starts_with("    ") {
                    let mut insert = Vec::new();
                    if !has_perms {
                        insert.push("    permissions:".to_string());
                        insert.push("      contents: write".to_string());
                    }
                    if !has_env {
                        insert.push("    environment: release".to_string());
                    }
                    for (j, new_line) in insert.into_iter().enumerate() {
                        lines.insert(i + j, new_line);
                    }
                    break;
                }
            }
        }
    }

    // Fix 4: move generated needs.plan expressions out of run blocks and into env vars
    for line in &mut lines {
        if line.contains("dist build ${{ needs.plan.outputs.tag-flag }} --print=linkage --output-format=json ${{ matrix.dist_args }} > dist-manifest.json") {
            *line = "          dist build ${NEEDS_PLAN_OUTPUTS_TAG_FLAG} --print=linkage --output-format=json ${{ matrix.dist_args }} > dist-manifest.json".to_string();
        } else if line.contains("dist build ${{ needs.plan.outputs.tag-flag }} --output-format=json \"--artifacts=global\" > dist-manifest.json") {
            *line = "          dist build ${NEEDS_PLAN_OUTPUTS_TAG_FLAG} --output-format=json \"--artifacts=global\" > dist-manifest.json".to_string();
        } else if line.contains("dist host ${{ needs.plan.outputs.tag-flag }} --steps=upload --steps=release --output-format=json > dist-manifest.json") {
            *line = "          dist host ${NEEDS_PLAN_OUTPUTS_TAG_FLAG} --steps=upload --steps=release --output-format=json > dist-manifest.json".to_string();
        } else if line.contains("gh release create \"${{ needs.plan.outputs.tag }}\" --target \"$RELEASE_COMMIT\" $PRERELEASE_FLAG --title \"$ANNOUNCEMENT_TITLE\" --notes-file \"$RUNNER_TEMP/notes.txt\" artifacts/*") {
            *line = "          gh release create \"${NEEDS_PLAN_OUTPUTS_TAG}\" --target \"$RELEASE_COMMIT\" $PRERELEASE_FLAG --title \"$ANNOUNCEMENT_TITLE\" --notes-file \"$RUNNER_TEMP/notes.txt\" artifacts/*".to_string();
        }
    }

    for i in 0..lines.len() {
        if lines[i] == "      - name: Build artifacts" && i + 1 < lines.len() {
            if lines[i + 1] == "        run: |" {
                lines.insert(i + 1, "        env:".to_string());
                lines.insert(
                    i + 2,
                    "          NEEDS_PLAN_OUTPUTS_TAG_FLAG: ${{ needs.plan.outputs.tag-flag }}"
                        .to_string(),
                );
            }
            break;
        }
    }

    for i in 0..lines.len() {
        if lines[i] == "      - id: cargo-dist" && i + 2 < lines.len() {
            if lines[i + 1] == "        shell: bash" && lines[i + 2] == "        run: |" {
                lines.insert(i + 2, "        env:".to_string());
                lines.insert(
                    i + 3,
                    "          NEEDS_PLAN_OUTPUTS_TAG_FLAG: ${{ needs.plan.outputs.tag-flag }}"
                        .to_string(),
                );
            }
            break;
        }
    }

    for i in 0..lines.len() {
        if lines[i] == "      - id: host" && i + 2 < lines.len() {
            if lines[i + 1] == "        shell: bash" && lines[i + 2] == "        run: |" {
                lines.insert(i + 2, "        env:".to_string());
                lines.insert(
                    i + 3,
                    "          NEEDS_PLAN_OUTPUTS_TAG_FLAG: ${{ needs.plan.outputs.tag-flag }}"
                        .to_string(),
                );
            }
            break;
        }
    }

    for i in 0..lines.len() {
        if lines[i] == "          RELEASE_COMMIT: \"${{ github.sha }}\"" {
            if i + 1 < lines.len()
                && lines[i + 1] != "          NEEDS_PLAN_OUTPUTS_TAG: ${{ needs.plan.outputs.tag }}"
            {
                lines.insert(
                    i + 1,
                    "          NEEDS_PLAN_OUTPUTS_TAG: ${{ needs.plan.outputs.tag }}".to_string(),
                );
            }
            break;
        }
    }

    // Fix 5: require protected release environment for Homebrew publish job
    let (start, end) = get_job_section(&lines, "publish-homebrew-formula");
    if let (Some(s), Some(e)) = (start, end) {
        let section_text = lines[s..e].join("\n");
        if !section_text.contains("environment: release") {
            for i in s..e {
                if lines[i].trim().starts_with("runs-on:") {
                    lines.insert(i + 1, "    environment: release".to_string());
                    break;
                }
            }
        }
    }

    // Fix 6: restore cargo-cyclonedx for release SBOM artifact
    let (start, end) = get_job_section(&lines, "build-global-artifacts");
    if let (Some(s), Some(e)) = (start, end) {
        let section_text = lines[s..e].join("\n");
        if !section_text.contains("Install cargo-cyclonedx") {
            for i in s..e {
                if lines[i] == "      - run: chmod +x ~/.cargo/bin/dist" {
                    lines.insert(i + 1, "      - name: Install cargo-cyclonedx".to_string());
                    lines.insert(i + 2, "        run: |".to_string());
                    lines.insert(
                        i + 3,
                        "          if ! cargo cyclonedx --version >/dev/null 2>&1; then"
                            .to_string(),
                    );
                    lines.insert(
                        i + 4,
                        "            cargo install cargo-cyclonedx --locked --version 0.5.9"
                            .to_string(),
                    );
                    lines.insert(i + 5, "          fi".to_string());
                    break;
                }
            }
        }
    }

    // Fix 7: preserve existing-release upload fallback used with release-please tags
    let simple_release = "          gh release create \"${NEEDS_PLAN_OUTPUTS_TAG}\" --target \"$RELEASE_COMMIT\" $PRERELEASE_FLAG --title \"$ANNOUNCEMENT_TITLE\" --notes-file \"$RUNNER_TEMP/notes.txt\" artifacts/*";
    for i in 0..lines.len() {
        if lines[i] == simple_release {
            lines.splice(
                i..=i,
                vec![
                    "          if gh release view \"${NEEDS_PLAN_OUTPUTS_TAG}\" >/dev/null 2>&1; then".to_string(),
                    "            gh release upload \"${NEEDS_PLAN_OUTPUTS_TAG}\" artifacts/* --clobber".to_string(),
                    "          else".to_string(),
                    "            gh release create \"${NEEDS_PLAN_OUTPUTS_TAG}\" --target \"$RELEASE_COMMIT\" $PRERELEASE_FLAG --title \"$ANNOUNCEMENT_TITLE\" --notes-file \"$RUNNER_TEMP/notes.txt\" artifacts/*".to_string(),
                    "          fi".to_string(),
                ],
            );
            break;
        }
    }

    let mut result = lines.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Run the release security check or fix.
pub fn run_check_security(fix: bool, workflow_path: Option<&Path>) -> Result<()> {
    let repo_root = find_repo_root()?;
    let path = workflow_path
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root.join(DEFAULT_WORKFLOW_PATH));

    print_section("Release Security Verification");
    print_step(&format!("Auditing workflow file: {}", path.display()));

    if !path.exists() {
        if workflow_path.is_none() {
            print_warning(&format!(
                "Default release workflow file not found at {}, skipping audit.",
                path.display()
            ));
            return Ok(());
        }
        bail!("Workflow file not found at: {}", path.display());
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;

    let issues = check_security_content(&content);

    if fix {
        if issues.is_empty() {
            print_success("Nothing to fix. Release workflow is already hardened.");
            return Ok(());
        }
        print_step(&format!(
            "Applying security hardening fixes for {} issues...",
            issues.len()
        ));
        let fixed = fix_security_content(&content);
        fs::write(&path, &fixed).with_context(|| format!("Failed to write {}", path.display()))?;

        let remaining = check_security_content(&fixed);
        if !remaining.is_empty() {
            print_error(&format!("Could not auto-fix {} issue(s):", remaining.len()));
            for issue in &remaining {
                eprintln!("  {} {issue}", "•".red());
            }
            bail!("Failed to auto-fix all security issues in release workflow");
        }
        print_success(&format!(
            "Successfully hardened {} issue(s) in {}",
            issues.len(),
            path.display()
        ));
        for issue in &issues {
            println!("  {} {issue}", "✔".green());
        }
        Ok(())
    } else {
        if !issues.is_empty() {
            print_error(&format!(
                "Found {} security hardening issue(s) in {}:",
                issues.len(),
                path.display()
            ));
            for issue in &issues {
                eprintln!("  {} {issue}", "•".red());
            }
            eprintln!("\n{}", "To automatically fix these issues, run:".yellow());
            eprintln!("  {}", "tooling release check-security --fix".cyan().bold());
            bail!("Release workflow security check failed");
        }
        print_success("All release workflow security hardening checks passed.");
        Ok(())
    }
}

/// Helper to get publishable workspace crates via `cargo metadata`.
pub fn get_publishable_workspace_crates(repo_root: &Path) -> Result<Vec<String>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(repo_root)
        .output()
        .context("Failed to run cargo metadata")?;

    if !output.status.success() {
        bail!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let meta: Value = serde_json::from_slice(&output.stdout)?;
    let packages = meta["packages"]
        .as_array()
        .context("Missing packages in metadata")?;

    let mut publishable = Vec::new();
    for pkg in packages {
        let publish = &pkg["publish"];
        let is_publishable = if let Some(arr) = publish.as_array() {
            !arr.is_empty()
        } else {
            // None means publish = true (default)
            true
        };

        if is_publishable && let Some(name) = pkg["name"].as_str() {
            publishable.push(name.to_string());
        }
    }
    publishable.sort();
    Ok(publishable)
}

/// Run check for `.release-please-config.json` crate and python mappings.
pub fn run_check_config() -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Release-Please Configuration Verification");

    let config_path = repo_root.join(".release-please-config.json");
    if !config_path.exists() {
        bail!(
            ".release-please-config.json not found at: {}",
            config_path.display()
        );
    }

    let config_text = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    let config: Value = serde_json::from_str(&config_text)
        .with_context(|| "Failed to parse .release-please-config.json as JSON")?;

    let extra_files = config["packages"]["."]["extra-files"]
        .as_array()
        .context("Missing packages.'.'extra-files in .release-please-config.json")?;

    let mut configured_jsonpaths = HashSet::new();
    let mut configured_generics = HashSet::new();

    for entry in extra_files {
        if let (Some(path), Some(jsonpath)) = (entry["path"].as_str(), entry["jsonpath"].as_str()) {
            configured_jsonpaths.insert(format!("{path}::{jsonpath}"));
        }
        if let (Some(path), Some(t)) = (entry["path"].as_str(), entry["type"].as_str())
            && t == "generic"
        {
            configured_generics.insert(path.to_string());
        }
    }

    let publishable_crates = get_publishable_workspace_crates(&repo_root)?;
    print_step(&format!(
        "Found {} publishable workspace crates.",
        publishable_crates.len()
    ));

    let mut expected = HashSet::new();
    for crate_name in &publishable_crates {
        expected.insert(format!(
            "Cargo.toml::$.workspace.dependencies['{crate_name}'].version"
        ));
    }
    expected.insert("python/pyproject.toml::$.project.version".to_string());

    let mut missing = Vec::new();
    for exp in &expected {
        if !configured_jsonpaths.contains(exp) {
            missing.push(exp.clone());
        }
    }

    if !configured_generics.contains("pyproject.toml") {
        missing.push("pyproject.toml::generic".to_string());
    }

    missing.sort();
    if !missing.is_empty() {
        print_error(&format!(
            "Missing {} release-please extra-files entries:",
            missing.len()
        ));
        for m in &missing {
            eprintln!("  {} {m}", "•".red());
        }
        bail!("release-please configuration verification failed");
    }

    let pyproject_path = repo_root.join("pyproject.toml");
    if pyproject_path.exists() {
        let pyproject_text = fs::read_to_string(&pyproject_path)?;
        let marker = "x-release-please-version";
        let marker_count = pyproject_text.matches(marker).count();
        if marker_count != 2 {
            print_error(&format!(
                "Expected pyproject.toml to contain 2 '{marker}' annotations, found {marker_count}"
            ));
            bail!("pyproject.toml release annotations check failed");
        }
    }

    print_success("Release-please config covers all Rust crates and Python packaging metadata.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_WORKFLOW: &str = r#"name: Release
permissions:
  contents: write

jobs:
  plan:
    runs-on: ubuntu-22.04
  build-global-artifacts:
    runs-on: ubuntu-22.04
    steps:
      - run: chmod +x ~/.cargo/bin/dist
      - id: cargo-dist
        shell: bash
        run: |
          dist build ${{ needs.plan.outputs.tag-flag }} --output-format=json "--artifacts=global" > dist-manifest.json
      - name: Build artifacts
        run: |
          dist build ${{ needs.plan.outputs.tag-flag }} --print=linkage --output-format=json ${{ matrix.dist_args }} > dist-manifest.json
  host:
    runs-on: ubuntu-22.04
    env:
      GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
    steps:
      - id: host
        shell: bash
        run: |
          dist host ${{ needs.plan.outputs.tag-flag }} --steps=upload --steps=release --output-format=json > dist-manifest.json
      - name: Create GitHub Release
        env:
          PRERELEASE_FLAG: "${{ fromJson(steps.host.outputs.manifest).announcement_is_prerelease && '--prerelease' || '' }}"
          ANNOUNCEMENT_TITLE: "${{ fromJson(steps.host.outputs.manifest).announcement_title }}"
          ANNOUNCEMENT_BODY: "${{ fromJson(steps.host.outputs.manifest).announcement_github_body }}"
          RELEASE_COMMIT: "${{ github.sha }}"
        run: |
          gh release create "${{ needs.plan.outputs.tag }}" --target "$RELEASE_COMMIT" $PRERELEASE_FLAG --title "$ANNOUNCEMENT_TITLE" --notes-file "$RUNNER_TEMP/notes.txt" artifacts/*
  publish-homebrew-formula:
    needs:
      - plan
      - host
    runs-on: ubuntu-22.04
    steps:
      - uses: actions/checkout@deadbeef
        with:
          token: ${{ secrets.HOMEBREW_TAP_TOKEN }}
"#;

    #[test]
    fn test_check_reports_new_release_workflow_risks() {
        let issues = check_security_content(SAMPLE_WORKFLOW);

        assert!(issues.contains(&"top-level permissions grants write access".to_string()));
        assert!(issues.contains(&"plan job missing per-job permissions".to_string()));
        assert!(issues.contains(&"host job missing per-job permissions".to_string()));
        assert!(issues.contains(&"host job missing environment: release".to_string()));
        assert!(
            issues.contains(&"dist build uses direct template expansion in run block".to_string())
        );
        assert!(issues.contains(
            &"host dist command uses direct template expansion in run block".to_string()
        ));
        assert!(
            issues.contains(
                &"release creation uses direct template expansion in run block".to_string()
            )
        );
        assert!(
            issues.contains(&"publish-homebrew-formula missing environment: release".to_string())
        );
        assert!(
            issues.contains(&"build-global-artifacts missing cargo-cyclonedx install".to_string())
        );
        assert!(issues.contains(
            &"release publishing no longer updates existing GitHub releases".to_string()
        ));
    }

    #[test]
    fn test_fix_hardens_generated_release_workflow() {
        let fixed = fix_security_content(SAMPLE_WORKFLOW);

        assert!(fixed.contains("    permissions:\n      contents: write"));
        assert!(fixed.contains("    environment: release"));
        assert!(fixed.contains(
            "          dist build ${NEEDS_PLAN_OUTPUTS_TAG_FLAG} --output-format=json \"--artifacts=global\" > dist-manifest.json"
        ));
        assert!(fixed.contains(
            "          dist host ${NEEDS_PLAN_OUTPUTS_TAG_FLAG} --steps=upload --steps=release --output-format=json > dist-manifest.json"
        ));
        assert!(
            fixed.contains(
                "          NEEDS_PLAN_OUTPUTS_TAG_FLAG: ${{ needs.plan.outputs.tag-flag }}"
            )
        );
        assert!(fixed.contains("          NEEDS_PLAN_OUTPUTS_TAG: ${{ needs.plan.outputs.tag }}"));
        assert!(fixed.contains("      - name: Install cargo-cyclonedx"));
        assert!(fixed.contains(
            "            gh release upload \"${NEEDS_PLAN_OUTPUTS_TAG}\" artifacts/* --clobber"
        ));
        assert!(fixed.contains(
            "          gh release create \"${NEEDS_PLAN_OUTPUTS_TAG}\" --target \"$RELEASE_COMMIT\" $PRERELEASE_FLAG --title \"$ANNOUNCEMENT_TITLE\" --notes-file \"$RUNNER_TEMP/notes.txt\" artifacts/*"
        ));

        let remaining = check_security_content(&fixed);
        assert_eq!(remaining, Vec::<String>::new());
    }
}
