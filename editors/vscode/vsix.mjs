import { spawnSync } from "node:child_process";
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
const result = spawnSync(process.execPath, [require.resolve("@vscode/vsce/vsce"), operation, "--no-dependencies", "--target", hostTarget(), ...args], { stdio: "inherit" });
if (result.error) {
  throw result.error;
}
process.exitCode = result.status ?? 1;
