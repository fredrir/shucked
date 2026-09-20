import * as fs from "node:fs";
import * as path from "node:path";
import * as cp from "node:child_process";
import { fileURLToPath } from "node:url";
import { binaryPlatform, hostTarget } from "./platform.mjs";
import { verifyProviderRuntime } from "./provider-artifacts.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");
const binDir = path.join(__dirname, "bin");

const isWin = process.platform === "win32";
const binaries = [
  isWin ? "shucked.exe" : "shucked",
  isWin ? "shucked-server.exe" : "shucked-server",
];

fs.mkdirSync(binDir, { recursive: true });

// Cargo's incremental build verifies freshness before every distribution.
const build = cp.spawnSync(
  "cargo", ["build", "--release", "-p", "shucked-cli", "-p", "shucked-server"],
  { cwd: repoRoot, stdio: "inherit" },
);
if (build.error) {
  throw build.error;
}
if (build.status !== 0) {
  throw new Error(`cargo build exited with status ${build.status}`);
}
function findSourceBinary(name) {
  return path.join(repoRoot, "target", "release", name);
}

for (const name of binaries) {
  const src = findSourceBinary(name);
  if (!src) {
    throw new Error(
      `Could not find required binary ${name} in target/release or target/debug. ` +
        `Please run 'cargo build --release -p shucked-cli -p shucked-server' first.`,
    );
  }
  const binary = binaryPlatform(src);
  if (binary?.platform !== process.platform || binary?.arch !== process.arch) {
    throw new Error(`Binary ${src} does not match ${hostTarget()}. Rebuild on the target host.`);
  }
  const dst = path.join(binDir, name);
  fs.copyFileSync(src, dst);
  if (!isWin) {
    fs.chmodSync(dst, 0o755);
  }
  const stat = fs.statSync(dst);
  console.log(
    `Bundled ${name} (${(stat.size / (1024 * 1024)).toFixed(2)} MB) -> ${dst}`,
  );
}

fs.writeFileSync(path.join(binDir, "platform.json"), JSON.stringify({ target: hostTarget() }) + "\n");

const providerSource = path.join(repoRoot, "tooling", "providers", "packs");
const providerRuntime = path.join(repoRoot, "target", "provider-runtime");
const providerDestination = path.join(binDir, "providers");
const packManifest = JSON.parse(fs.readFileSync(path.join(providerSource, "manifest.json"), "utf8"));
const crypto = await import("node:crypto");
for (const file of [...packManifest.sources.flatMap(source => source.files), ...(packManifest.generated ?? [])]) {
  const bytes = fs.readFileSync(path.join(providerSource, file.path));
  if (crypto.createHash("sha256").update(bytes).digest("hex") !== file.sha256) {
    throw new Error(`Provider pack checksum mismatch: ${file.path}`);
  }
}
if (!fs.existsSync(path.join(providerRuntime, "manifest.json"))) {
  throw new Error("Provider runtimes missing. Run tooling/providers/build-zsh.sh and bundle-runtime.py before packaging.");
}
const runtimeManifest = verifyProviderRuntime(providerRuntime, hostTarget());
fs.rmSync(providerDestination, { recursive: true, force: true });
fs.mkdirSync(providerDestination, { recursive: true });
fs.cpSync(providerSource, path.join(providerDestination, "packs"), { recursive: true });
fs.cpSync(providerRuntime, path.join(providerDestination, "runtime"), { recursive: true, dereference: true });
console.log(`Bundled ${packManifest.sources.length} completion packs and ${runtimeManifest.sources.length} runtime source packages`);
