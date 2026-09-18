use anyhow::Result;

use crate::runner::{
    RunOptions, find_repo_root, is_tool_available, print_section, print_step, print_success,
    print_warning, run_command,
};

/// Run build commands according to selected targets.
pub fn run_build(
    release: bool,
    wasm: bool,
    cli: bool,
    server: bool,
    all: bool,
    extra_args: &[String],
) -> Result<()> {
    let repo_root = find_repo_root()?;
    print_section("Build Workspace");

    let opts = RunOptions {
        cwd: Some(&repo_root),
        ..Default::default()
    };

    let build_wasm_target = wasm || all;
    let build_cli_target = cli || (all && !wasm && !server);
    let build_server_target = server || (all && !wasm && !cli);
    let build_workspace = !wasm && !cli && !server;

    if build_workspace || all {
        print_step("Building workspace Rust crates...");
        let mut args = vec!["build"];
        if release {
            args.push("--release");
        }
        for extra in extra_args {
            args.push(extra);
        }
        run_command("cargo", &args, &opts)?;
    } else {
        if build_cli_target {
            print_step("Building shucked-cli...");
            let mut args = vec!["build", "-p", "shucked-cli"];
            if release {
                args.push("--release");
            }
            for extra in extra_args {
                args.push(extra);
            }
            run_command("cargo", &args, &opts)?;
        }

        if build_server_target {
            print_step("Building shucked-server and shucked-lsp...");
            let mut args = vec!["build", "-p", "shucked-server", "-p", "shucked-lsp"];
            if release {
                args.push("--release");
            }
            for extra in extra_args {
                args.push(extra);
            }
            run_command("cargo", &args, &opts)?;
        }
    }

    if build_wasm_target {
        print_step("Building WebAssembly target (shucked-wasm)...");
        if is_tool_available("wasm-pack") {
            let wasm_args = [
                "build",
                "crates/shucked-wasm",
                "--target",
                "bundler",
                "--out-dir",
                "../../target/npm/shucked-wasm",
                "--out-name",
                "shuck",
            ];
            run_command("wasm-pack", &wasm_args, &opts)?;
        } else {
            print_warning(
                "wasm-pack not detected on PATH, falling back to cargo build for shucked-wasm.",
            );
            let mut args = vec!["build", "-p", "shucked-wasm"];
            if release {
                args.push("--release");
            }
            run_command("cargo", &args, &opts)?;
        }
    }

    print_success("Build completed successfully.");
    Ok(())
}
