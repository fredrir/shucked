import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const compiled = await build({
  entryPoints: [fileURLToPath(new URL("../src/binary.ts", import.meta.url))],
  bundle: true,
  platform: "node",
  format: "cjs",
  external: ["vscode"],
  write: false,
});

function resolver(environment, config = {}) {
  const module = { exports: {} };
  runInNewContext(compiled.outputFiles[0].text, {
    module,
    exports: module.exports,
    Buffer,
    process: environment,
    require: (id) => id === "vscode"
      ? { workspace: { getConfiguration: () => ({ get: (name, fallback) => config[name] ?? fallback }) } }
      : require(id),
  });
  return module.exports;
}

test("remote ARM host skips a desktop bundle and resolves its own PATH", async () => {
  const root = mkdtempSync(join(tmpdir(), "shucked-remote-"));
  const extensionPath = join(root, "editors", "vscode");
  const remoteBin = join(root, "remote-bin");
  mkdirSync(join(extensionPath, "bin"), { recursive: true });
  mkdirSync(remoteBin);
  try {
    const desktop = Buffer.alloc(32);
    desktop.writeUInt32LE(0xfeedfacf, 0);
    desktop.writeUInt32LE(0x0100000c, 4);
    writeFileSync(join(extensionPath, "bin", "shucked-server"), desktop, { mode: 0o755 });
    const remote = Buffer.alloc(64);
    remote.set([0x7f, 0x45, 0x4c, 0x46, 2, 1]);
    remote.writeUInt16LE(183, 18);
    const command = join(remoteBin, "shucked-server");
    writeFileSync(command, remote, { mode: 0o755 });
    const messages = [];
    const output = { info: (message) => messages.push(message), warn: (message) => messages.push(message) };
    const api = resolver({ platform: "linux", arch: "arm64", env: { PATH: remoteBin } });
    const result = await api.resolveServerCommand({ extensionPath }, output, ["--test-arg"]);
    assert.equal(result.command, command);
    assert.ok(messages.some((message) => message.includes("Skipping incompatible bundled binary")));
    assert.deepEqual(Array.from(result.args), ["--test-arg"]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("discovery does not grant executable permissions to a custom binary", { skip: process.platform === "win32" }, () => {
  const root = mkdtempSync(join(tmpdir(), "shucked-permissions-"));
  try {
    const file = join(root, "custom-server");
    writeFileSync(file, "not executable", { mode: 0o600 });
    const api = resolver(process);
    assert.equal(api.ensureExecutable(file), false);
    assert.equal(statSync(file).mode & 0o777, 0o600);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
