import * as fs from "node:fs";
import * as path from "node:path";
import * as cp from "node:child_process";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");
const binDir = path.join(__dirname, "bin");

const isWin = process.platform === "win32";
const binaries = [
  isWin ? "shucked.exe" : "shucked",
  isWin ? "shucked-server.exe" : "shucked-server",
];

fs.mkdirSync(binDir, { recursive: true });

function findSourceBinary(name) {
  const releasePath = path.join(repoRoot, "target", "release", name);
  if (fs.existsSync(releasePath)) {
    return releasePath;
  }
  const debugPath = path.join(repoRoot, "target", "debug", name);
  if (fs.existsSync(debugPath)) {
    return debugPath;
  }
  return null;
}

const missing = binaries.some((name) => !findSourceBinary(name));

if (missing) {
  console.log("Building shucked and shucked-server binaries via cargo...");
  const res = cp.spawnSync(
    "cargo",
    ["build", "--release", "-p", "shucked-cli", "-p", "shucked-server"],
    {
      cwd: repoRoot,
      stdio: "inherit",
    },
  );
  if (res.status !== 0) {
    console.warn(`cargo build exited with status ${res.status}`);
  }
}

for (const name of binaries) {
  const src = findSourceBinary(name);
  if (!src) {
    throw new Error(
      `Could not find required binary ${name} in target/release or target/debug. ` +
        `Please run 'cargo build --release -p shucked-cli -p shucked-server' first.`,
    );
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
