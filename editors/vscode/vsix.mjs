import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { hostTarget } from "./platform.mjs";

const require = createRequire(import.meta.url);
const operation = process.argv[2] ?? "package";
if (!["package", "publish"].includes(operation)) {
  throw new Error(`Unsupported VSIX operation: ${operation}`);
}
const args = process.argv.slice(3);
if (args.some((arg) => arg === "--target" || arg.startsWith("--target="))) {
  throw new Error("Build platform packages on their target host; --target overrides are disabled.");
}
const started = performance.now();
console.error(`Starting VSIX ${operation}; building, validating, and compressing the bundled runtimes can take a few minutes.`);
const child = spawn(process.execPath, [require.resolve("@vscode/vsce/vsce"), operation, "--no-dependencies", "--target", hostTarget(), ...args], { stdio: "inherit" });
const progress = setInterval(() => {
  const elapsed = Math.floor((performance.now() - started) / 1000);
  console.error(`VSIX ${operation} still running (${elapsed}s elapsed)...`);
}, 10_000);
const interrupt = () => child.kill("SIGINT");
const terminate = () => child.kill("SIGTERM");
process.on("SIGINT", interrupt);
process.on("SIGTERM", terminate);
child.on("error", (error) => {
  console.error(`Failed to start vsce: ${error.message}`);
});
child.on("close", (code) => {
  clearInterval(progress);
  process.removeListener("SIGINT", interrupt);
  process.removeListener("SIGTERM", terminate);
  process.exitCode = code ?? 1;
});
