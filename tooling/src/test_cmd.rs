use anyhow::Result;
use std::time::Instant;

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
};

/// Run test suites according to specified targets.
#[allow(clippy::too_many_arguments)]
pub fn run_test(
    unit: bool,
    lsp: bool,
    linter: bool,
    python: bool,
    wasm: bool,
    all: bool,
    release: bool,
    package: Option<&str>,
    filter: Option<&str>,
    extra_args: &[String],
) -> Result<()> {
    let repo_root = find_repo_root()?;
    let start_total = Instant::now();
    print_section("Test Runner");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    // If a specific package was requested, test just that package
    if let Some(pkg) = package {
        print_step(&format!("Running tests for package `{}`...", pkg));
        let mut args = vec!["test", "-p", pkg];
        if release {
            args.push("--release");
        }
        for extra in extra_args {
            args.push(extra);
        }
        if let Some(f) = filter {
            args.push(f);
        }
        run_command("cargo", &args, &opts)?;
        print_success(&format!(
            "Package `{}` tests passed in {:.2?}.",
            pkg,
            start_total.elapsed()
        ));
        return Ok(());
    }

    // Map recognized suite names passed positionally into their corresponding test suite
    let (filter, python, lsp, linter, wasm, all) = match filter {
        Some("python") | Some("py") => (None, true, lsp, linter, wasm, all),
        Some("lsp") => (None, python, true, linter, wasm, all),
        Some("linter") => (None, python, lsp, true, wasm, all),
        Some("wasm") => (None, python, lsp, linter, true, all),
        Some("all") => (None, python, lsp, linter, wasm, true),
        other => (other, python, lsp, linter, wasm, all),
    };

    let run_all = all;
    let run_unit = unit || run_all || (!lsp && !linter && !python && !wasm);
    let run_lsp = lsp || run_all;
    let run_linter = linter || run_all;
    let run_python = python || run_all;
    let run_wasm = wasm || run_all;

    // 1. Rust Unit Tests
    if run_unit && !run_lsp && !run_linter {
        print_step("Running workspace Rust unit tests...");
        let mut args = vec!["test", "--workspace", "--exclude", "shucked-wasm"];
        if release {
            args.push("--release");
        }
        for extra in extra_args {
            args.push(extra);
        }
        if let Some(f) = filter {
            args.push(f);
        }
        run_command("cargo", &args, &opts)?;
    }

    // 2. LSP Tests
    if run_lsp {
        print_step("Running shucked-lsp tests...");
        let mut args = vec!["test", "-p", "shucked-lsp"];
        if release {
            args.push("--release");
        }
        for extra in extra_args {
            args.push(extra);
        }
        if let Some(f) = filter {
            args.push(f);
        }
        run_command("cargo", &args, &opts)?;
    }

    // 3. Linter Tests
    if run_linter {
        print_step("Running shucked-linter tests...");
        let mut args = vec!["test", "-p", "shucked-linter"];
        if release {
            args.push("--release");
        }
        for extra in extra_args {
            args.push(extra);
        }
        if let Some(f) = filter {
            args.push(f);
        }
        run_command("cargo", &args, &opts)?;
    }

    // 4. Python Tests
    if run_python {
        print_step("Running Python test suite via uv...");
        if is_tool_available("uv") {
            let py_args = ["run", "--project", "tests", "pytest", "tests/", "-v"];
            run_command("uv", &py_args, &opts)?;
        } else {
            print_warning("uv is not installed on PATH, skipping Python tests.");
        }
    }

    // 5. WASM Tests
    if run_wasm {
        print_step("Running WebAssembly tests...");
        let mut wasm_args = vec!["test", "-p", "shucked-wasm"];
        if release {
            wasm_args.push("--release");
        }
        run_command("cargo", &wasm_args, &opts)?;

        if is_tool_available("wasm-pack") && is_tool_available("node") {
            print_step("Building nodejs wasm package & running smoke test...");
            let build_args = [
                "build",
                "crates/shucked-wasm",
                "--target",
                "nodejs",
                "--out-dir",
                "../../target/wasm-test/shucked-wasm",
                "--out-name",
                "shuck",
            ];
            run_command("wasm-pack", &build_args, &opts)?;

            let node_code = r#"
const assert = require("node:assert/strict");
const path = require("node:path");
const pkgDir = path.resolve(process.argv[1]);
const shuck = require(pkgDir);
const pkgJson = require(path.join(pkgDir, "package.json"));
const diagnostics = shuck.lint("echo $name\n", { filename: "script.bash", select: ["ALL"] });
assert.ok(Array.isArray(diagnostics));
assert.ok(Array.isArray(shuck.lint("echo ok\n")));
assert.equal(shuck.version(), pkgJson.version);
console.log(`smoke-tested ${pkgJson.name}@${pkgJson.version}`);
"#;
            let target_wasm = "target/wasm-test/shucked-wasm";
            let node_args = ["-e", node_code, target_wasm];
            run_command("node", &node_args, &opts)?;
        }
    }

    print_success(&format!(
        "All requested tests passed in {:.2?}.",
        start_total.elapsed()
    ));
    Ok(())
}
